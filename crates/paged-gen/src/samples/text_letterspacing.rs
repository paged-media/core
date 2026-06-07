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

//! Cycle-7 mega-file: `text-letterspacing.idml`.
//!
//! Single-purpose calibration fixture. Two pages: a control paragraph
//! at default letter-spacing and a tuned paragraph that carries
//! non-default `Min/Desired/MaxLetterSpacing` matching what real
//! InDesign body styles ship with. Both render the same real-English
//! body text so a Q-20 calibration tweak that changes wrap decisions
//! is detectable by the break-decision self-diff harness
//! (`corpus/generated/breaks-diff.sh`).
//!
//! Pinned in `corpus/generated/text-letterspacing.breaks.jsonl` —
//! the in-tree BreakRecord snapshot used as the self-diff reference.
//! No companion PDF; this fixture intentionally lives outside the
//! pixel-fidelity gate (`corpus/generated/diff.sh`) so it doesn't
//! need an InDesign export round-trip.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text-letterspacing";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
// Cycle-8 Track 4: tightened from 200pt to 150pt — at 12pt Open Sans
// (~6pt/char average) a 150pt column wraps ~13 lines per page with
// natural-break widths landing close to the column edge ([127, 151]pt
// in the pinned snapshot). That's the regime where the Q-20 LS
// stretch/shrink budget actually decides whether a candidate break
// point is feasible; the original 200pt column had too much slack for
// any knob change to matter.
const FRAME_W_PT: f32 = 150.0;
const FRAME_H_PT: f32 = 720.0;

/// Real-English filler so the breaker has real word choices to make.
/// Length tuned to wrap ~13 lines at 12pt in the 150pt column.
const BODY_TEXT: &str = "The quick brown fox jumps over the lazy dog. \
    Pack my box with five dozen liquor jugs. \
    Sphinx of black quartz, judge my vow. \
    How vexingly quick daft zebras jump. \
    Bright vixens jump; dozy fowl quack. \
    A wizard's job is to vex chumps quickly in fog. \
    Watch Jeopardy! Alex Trebek's fun TV quiz game. \
    Five quacking zephyrs jolt my wax bed.";

struct Variant {
    name: &'static str,
    /// LS attribute spread on the paragraph style range. `None` means
    /// no LS attrs at all (default — the control paragraph).
    letter_spacing: Option<(f32, f32, f32)>, // (min, desired, max)
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "control · default-LS",
            letter_spacing: None,
        },
        Variant {
            // Matches the spread on newspaper's `Body, Left Justify`
            // paragraph style — a representative real-world LS budget.
            // Q-20 calibration knob changes (LS_BUDGET_PT_FOR_FULL_STRETCH)
            // should shift this fixture's break decisions.
            name: "tuned · LS-spread-25",
            letter_spacing: Some((-5.0, 0.0, 25.0)),
        },
    ]
}

fn body_paragraph(ls: Option<(f32, f32, f32)>) -> Paragraph {
    Paragraph {
        justification: Some("LeftAlign"),
        space_before: None,
        space_after: None,
        leading: None,
        first_line_indent: None,
        left_indent: None,
        right_indent: None,
        drop_cap_characters: None,
        drop_cap_lines: None,
        tab_list: Vec::new(),
        bullets_list_type: None,
        applied_numbering_list: None,
        bullet_character: None,
        table: None,
        minimum_letter_spacing: ls.map(|(a, _, _)| a),
        desired_letter_spacing: ls.map(|(_, b, _)| b),
        maximum_letter_spacing: ls.map(|(_, _, c)| c),
        runs: vec![Run {
            text: BODY_TEXT.to_string(),
            point_size: Some(12.0),
            fill_color: None,
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: None,
            anchored_frame: None,
        }],
    }
}

pub fn build() -> Sample {
    let variants = variants();
    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len());

    for (i, variant) in variants.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let body_story_id = self_id(SAMPLE, "BodyStory", seq);
        let body_frame_id = self_id(SAMPLE, "BodyFrame", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id,
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        stories.push((
            body_story_id.clone(),
            write_story(&Story {
                self_id: body_story_id.clone(),
                paragraphs: vec![body_paragraph(variant.letter_spacing)],
            }),
        ));
        story_refs.push(body_story_id.clone());

        let body_x = (PAGE_W_PT - FRAME_W_PT) * 0.5;
        let body_y = 60.0;
        let body = Rect {
            self_id: body_frame_id,
            width_pt: FRAME_W_PT,
            height_pt: FRAME_H_PT,
            item_transform: translate(body_x, body_y),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(body_story_id),
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
        };

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: variant.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![body.into()],
                override_list: Vec::new(),
                margins: None,
            }),
        ));
        spread_refs.push(spread_id);
    }

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: master_refs,
        spreads: spread_refs,
        stories: story_refs,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
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
