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

//! `justified-probe` — the paragraphs the justified breaker was fitted on
//! (ADR 144): ten paragraphs of plain prose in eight frame widths, 120 to
//! 260 pt, hyphenated on page 1 and not on page 2; Open Sans 10/12,
//! `LeftJustified`, zero insets. InDesign 20.0.1's lines for each are in
//! `tools/indesign-export/justified-lines-probe.json` (asked through its
//! DOM by `justified-lines-probe.jsx`, which sets the same paragraphs), and
//! `paged-renderer`'s `justified_lines` test compares them. Not a gate
//! fixture: `emit-all` leaves it out; `paged-gen emit --sample
//! justified-probe` writes it.

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

const SAMPLE: &str = "justified-probe";
const PAGE_W_PT: f32 = 2400.0;
const PAGE_H_PT: f32 = 4300.0;
const BODY_FONT: &str = "Open Sans";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
const FRAME_H: f32 = 400.0;
const ROW_PITCH: f32 = 420.0;
const COL_GAP: f32 = 12.0;

/// The frame widths, left to right.
pub const WIDTHS: [f32; 8] = [120.0, 140.0, 160.0, 180.0, 200.0, 220.0, 240.0, 260.0];

/// The paragraphs, top to bottom.
/// The paragraphs, plain prose of mixed word lengths. The InDesign probe
/// (`tools/indesign-export/justified-lines-probe.jsx`) sets the same ten.
pub const TEXTS: [&str; 10] = [
    "Printing from movable type changed how knowledge travelled, because a page set once \
     could be pressed hundreds of times before the forme was broken up and the letters \
     returned to their cases. Compositors learned to judge a line by eye, adding thin \
     spaces between words or squeezing them tighter until every line filled its measure.",
    "A justified column asks more of its words than a ragged one. Each line must reach both \
     margins, so the spaces stretch and shrink, and long words near the end of a line may \
     be divided. Good composition balances these adjustments across the whole paragraph \
     instead of fixing each line in turn, and the reader never notices.",
    "Considerable experimentation showed that international typographic conventions, \
     documentation requirements and characteristically unpredictable manuscripts demand \
     extraordinarily careful hyphenation, particularly within narrow newspaper columns \
     where consecutive hyphenated lines quickly become distracting.",
    "Short words fit anywhere: a cat, a dog, an owl and a fox sat in a row by the old red \
     barn, and one by one they got up, ran off and hid in the tall wet grass at the far end \
     of the yard until the sun went down and the moon came up.",
    "The harbour master kept a ledger of every vessel that entered the bay, noting its \
     tonnage, its cargo and the name of its captain in a careful copperplate hand that grew \
     smaller each winter as the light in his office failed earlier in the afternoon.",
    "When the committee finally published its recommendations, the newspapers reported them \
     in very different ways: some praised the thoroughness of the investigation, others \
     complained that the conclusions were vague, and a few simply printed the summary \
     without any comment at all.",
    "Bread rises because yeast feeds on the sugars in the flour and releases carbon \
     dioxide, which the stretched gluten traps in countless small pockets; bake it too \
     early and the crumb is dense, too late and the loaf collapses as it cools on the rack.",
    "Mountain railways climb steep gradients with the help of a toothed rack laid between \
     the rails, so that a cogwheel beneath the locomotive can grip it firmly; passengers \
     often notice the gentle clicking long before they see the summit station appear above \
     the clouds.",
    "In the archive, letters from three centuries lie side by side: invoices, love notes, \
     legal summonses and recipes, each folded small and tied with ribbon, waiting for a \
     reader patient enough to untangle the handwriting and the relationships it records.",
    "Typographers distinguish between the measure, the length of a line of text, and the \
     column, the block of lines it forms; a comfortable measure holds between forty-five \
     and seventy-five characters, and anything much longer tires the eye as it returns to \
     the left margin.",
];

/// The story of the frame for `text` at width `col` on `page` (0 hyphenates).
pub fn story_id(page: u32, text: u32, col: u32) -> String {
    self_id(
        SAMPLE,
        "Story",
        (page * TEXTS.len() as u32 + text) * WIDTHS.len() as u32 + col,
    )
}

fn paragraph(text: &str, hyphenate: bool) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![
            ("Justification", "LeftJustified"),
            ("Hyphenation", if hyphenate { "true" } else { "false" }),
        ],
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
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut spreads = Vec::new();
    for page in 0..2u32 {
        let spread_id = self_id(SAMPLE, "Spread", page);
        let mut items: Vec<PageItem> = Vec::new();
        for (t, text) in TEXTS.iter().enumerate() {
            let mut x = 36.0;
            for (c, width) in WIDTHS.iter().enumerate() {
                let id = story_id(page, t as u32, c as u32);
                stories.push((
                    id.clone(),
                    write_story(&Story {
                        extra_story_attrs: Vec::new(),
                        self_id: id.clone(),
                        paragraphs: vec![paragraph(text, page == 0)],
                    }),
                ));
                story_refs.push(id.clone());
                items.push(
                    Rect {
                        self_id: self_id(
                            SAMPLE,
                            "Frame",
                            (page * TEXTS.len() as u32 + t as u32) * WIDTHS.len() as u32 + c as u32,
                        ),
                        width_pt: *width,
                        height_pt: FRAME_H,
                        item_transform: translate(x, 36.0 + t as f32 * ROW_PITCH),
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
                x += width + COL_GAP;
            }
        }
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id,
                page_self_id: self_id(SAMPLE, "Page", page),
                page_name: (page + 1).to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        ));
    }
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
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
        spreads,
        stories,
    }
}
