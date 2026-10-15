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

//! `numbering-multilevel.idml` — a numbered list with levels, asked of
//! InDesign (L-07).
//!
//! One story, one frame, every paragraph on the `NumberingList/Outline`
//! list through one of four paragraph styles:
//!
//! * `L1` — level 1, `1, 2, 3, 4...`, expression `^1.^t`;
//! * `L2` — level 2, `a, b, c, d...`, `^1.^2.^t`, restarting after any
//!   previous level (`NumberingRestartPolicies RestartPolicy=
//!   "AnyPreviousLevel"`);
//! * `L3` — level 3, `i, ii, iii, iv...`, `^1.^2.^3.^t`, restarting
//!   likewise;
//! * `L2Keep` — level 2, `1, 2, 3, 4...`, `^1-^2^t`, its restart policy
//!   off (`NumberingApplyRestartPolicy="false"`).
//!
//! The paragraphs go down and up the levels, skip level 2 once, and set
//! the no-restart level in two chapters, so the export shows how InDesign
//! counts each level, what `^n` prints and in which level's format, and
//! when a level starts again.

use crate::builders::{
    designmap::{write_designmap_with_markers, DesignMap, MarkerResources, NumberingListSpec},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "numbering-multilevel";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const LIST: &str = "NumberingList/Outline";

pub const L1: &str = "ParagraphStyle/L1";
pub const L2: &str = "ParagraphStyle/L2";
pub const L3: &str = "ParagraphStyle/L3";
pub const L2_KEEP: &str = "ParagraphStyle/L2Keep";

/// The paragraphs: `(style, text)`.
pub fn paragraphs() -> Vec<(&'static str, &'static str)> {
    vec![
        (L1, "Chapter one"),
        (L2, "Section"),
        (L2, "Section"),
        (L3, "Point"),
        (L3, "Point"),
        (L2, "Section"),
        (L3, "Point"),
        (L1, "Chapter two"),
        (L3, "Point under no section"),
        (L2, "Section"),
        (L1, "Chapter three"),
        (L2_KEEP, "Section that does not restart"),
        (L2_KEEP, "Section that does not restart"),
        (L1, "Chapter four"),
        (L2_KEEP, "Section that does not restart"),
        (L2, "Section"),
    ]
}

fn style(self_id: &str, name: &str, level: u32, expr: &str, format: &str, restart: bool) -> String {
    let policy = if level > 1 {
        "<NumberingRestartPolicies RestartPolicy=\"AnyPreviousLevel\" LowerLevel=\"0\" UpperLevel=\"0\"/>"
    } else {
        ""
    };
    format!(
        "<ParagraphStyle Self=\"{self_id}\" Name=\"{name}\" BulletsAndNumberingListType=\"NumberedList\" \
NumberingLevel=\"{level}\" NumberingExpression=\"{expr}\" NumberingContinue=\"true\" \
NumberingApplyRestartPolicy=\"{restart}\" LeftIndent=\"{indent}\" FirstLineIndent=\"-36\">\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn>\
<AppliedFont type=\"string\">{BODY_FONT}</AppliedFont>\
<AppliedNumberingList type=\"object\">{LIST}</AppliedNumberingList>\
<NumberingFormat type=\"string\">{format}</NumberingFormat>{policy}</Properties>\
</ParagraphStyle>",
        indent = 36 * level,
    )
}

fn styles_fragment() -> String {
    format!(
        "<RootParagraphStyleGroup>{}{}{}{}</RootParagraphStyleGroup>",
        style(L1, "L1", 1, "^1.^t", "1, 2, 3, 4...", true),
        style(L2, "L2", 2, "^1.^2.^t", "a, b, c, d...", true),
        style(L3, "L3", 3, "^1.^2.^3.^t", "i, ii, iii, iv...", true),
        style(L2_KEEP, "L2Keep", 2, "^1-^2^t", "1, 2, 3, 4...", false),
    )
}

fn paragraph(style: &'static str, text: &str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("AppliedParagraphStyle", style), ("Hyphenation", "false")],
        leading: Some(14.0),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(11.0),
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
    let story_id = self_id(SAMPLE, "Story", 0);
    let story = write_story(&Story {
        extra_story_attrs: Vec::new(),
        self_id: story_id.clone(),
        paragraphs: paragraphs()
            .into_iter()
            .map(|(s, t)| paragraph(s, t))
            .collect(),
    });
    let frame: PageItem = Rect {
        self_id: self_id(SAMPLE, "Frame", 0),
        width_pt: 420.0,
        height_pt: 600.0,
        item_transform: translate(88.0, 90.0),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story_id.clone()),
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
            inset_spacing: Some([0.0; 4]),
            first_baseline_offset: Some("AscentOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into();
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap_with_markers(
            &DesignMap {
                self_id: "d".to_string(),
                master_spreads: vec![master_id.clone()],
                spreads: vec![spread_id.clone()],
                stories: vec![story_id.clone()],
            },
            &MarkerResources {
                numbering_lists: vec![NumberingListSpec {
                    self_id: LIST.to_string(),
                    name: "Outline".to_string(),
                    continue_across_stories: false,
                    continue_across_documents: false,
                }],
                ..MarkerResources::default()
            },
        ),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml_with_raw(&styles_fragment()),
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
                page_items: vec![frame],
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        )],
        stories: vec![(story_id, story)],
    }
}
