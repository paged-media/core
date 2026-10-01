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

//! `forced-line-break.idml` — the forced line break (U+2028, InDesign's
//! Shift+Enter) against InDesign.
//!
//! A forced line break ends a LINE inside a paragraph: the next line is
//! the same paragraph's, so it takes no first-line indent, no space
//! before, no list marker, and the paragraph's space after waits for its
//! last line. Each case is one frame on one page with every paragraph in
//! its own `<ParagraphStyleRange>`, so an interior break in the fixture's
//! text is always a forced line break, never a paragraph mark.
//!
//! The line grid is the `keeps` fixture's: Inter 10/12, a `LeadingOffset`
//! first baseline and zero insets, so baseline *k* of a frame sits at
//! `12·k` pt below its top plus whatever spacing the case adds.

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

const SAMPLE: &str = "forced-line-break";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
pub const FRAME_W: f32 = 200.0;
const FRAME_H: f32 = 150.0;

/// U+2028 — the forced line break.
pub const LSEP: char = '\u{2028}';

/// One paragraph of a case. `text` uses `|` for a forced line break.
pub struct Para {
    pub text: &'static str,
    pub first_line_indent: Option<f32>,
    pub left_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub list: Option<&'static str>,
    pub justification: Option<&'static str>,
}

const fn p(text: &'static str) -> Para {
    Para {
        text,
        first_line_indent: None,
        left_indent: None,
        space_before: None,
        space_after: None,
        list: None,
        justification: None,
    }
}

pub struct Case {
    pub name: &'static str,
    pub paragraphs: Vec<Para>,
}

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    let hanging = |text, list| Para {
        left_indent: Some(18.0),
        first_line_indent: Some(-18.0),
        list: Some(list),
        ..p(text)
    };
    vec![
        Case {
            name: "indent + spacing",
            paragraphs: vec![
                Para {
                    first_line_indent: Some(24.0),
                    space_after: Some(10.0),
                    ..p("Alpha one|Bravo two|Charlie three")
                },
                Para {
                    first_line_indent: Some(24.0),
                    space_before: Some(8.0),
                    ..p("Delta four")
                },
            ],
        },
        Case {
            name: "bullets",
            paragraphs: vec![
                hanging("Echo five|Foxtrot six", "BulletList"),
                hanging("Golf seven", "BulletList"),
            ],
        },
        Case {
            name: "numbers",
            paragraphs: vec![
                hanging("Hotel eight|India nine", "NumberedList"),
                hanging("Juliet ten", "NumberedList"),
            ],
        },
        Case {
            name: "two breaks in a row",
            paragraphs: vec![p("Kilo eleven||Lima twelve"), p("Mike thirteen")],
        },
        Case {
            name: "break ends the paragraph",
            paragraphs: vec![p("November fourteen|"), p("Oscar fifteen")],
        },
        Case {
            name: "justified",
            paragraphs: vec![
                Para {
                    justification: Some("LeftJustified"),
                    ..p("Papa quebec romeo|Sierra tango")
                },
                p("Uniform sixteen"),
            ],
        },
        Case {
            name: "break starts the paragraph",
            paragraphs: vec![p("|Victor seventeen"), p("Whiskey eighteen")],
        },
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// Top-left of case `i`'s body frame, page-local pt.
pub fn frame_origin(i: u32) -> (f32, f32) {
    let col = i % 2;
    let row = i / 2;
    (72.0 + col as f32 * 240.0, 72.0 + row as f32 * 190.0)
}

fn paragraph(para: &Para) -> Paragraph {
    let text: String = para
        .text
        .chars()
        .map(|c| if c == '|' { LSEP } else { c })
        .collect();
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(LEADING),
        first_line_indent: para.first_line_indent,
        left_indent: para.left_indent,
        space_before: para.space_before,
        space_after: para.space_after,
        bullets_list_type: para.list,
        justification: para.justification,
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

fn frame(id: String, story: String, w: f32, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        stroke_color: body.then(|| "Color/Black".to_string()),
        stroke_weight_pt: body.then_some(0.25),
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
    let mut items = Vec::new();

    for (i, case) in cases().into_iter().enumerate() {
        let seq = i as u32;
        let (x, y) = frame_origin(seq);
        let label_story = self_id(SAMPLE, "LabelStory", seq);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph::plain(case.name)],
            }),
        ));
        story_refs.push(label_story.clone());
        items.push(frame(
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
                paragraphs: case.paragraphs.iter().map(paragraph).collect(),
            }),
        ));
        story_refs.push(body.clone());
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            FRAME_W,
            FRAME_H,
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
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: self_id(SAMPLE, "Page", 0),
        page_name: "1".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: items,
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

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
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
