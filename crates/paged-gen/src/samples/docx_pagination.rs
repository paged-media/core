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

//! `docx-pagination.idml` — the shape thoughts ADR 029 lowers a Word document
//! to, built to match plugin-doc's `pagination_docx()` fixture, whose
//! pagination WORD reported (`plugin-doc/scripts/word-pagination-probe.sh`,
//! Word 16, 2026-10-01):
//!
//! | page | size | lines | paragraphs |
//! |---|---|---|---|
//! | 1 | Letter | 53 | S1 P001-P053 |
//! | 2 | Letter | 54 | S1 P054-P107 (P054 carries keepNext) |
//! | 3 | Letter | 13 | S1 P108-P120 |
//! | 4 | A5 landscape | 28 | S2 P001-P028 |
//! | 5 | A5 landscape | 12 | S2 P029-P040 |
//!
//! The lowering: EACH Word section is its own story on its own page, whose
//! frame is the section's margin box, with a grow rule. Growth appends after
//! a story's last frame, so section 1's pages land before section 2's,
//! exactly where Word puts them. Only the authored first page of each
//! section is in this file; the grow rule is set by the test (IDML cannot
//! carry it), with `copy_frame_options` so generated pages keep the
//! section frame's `LeadingOffset` (Word's line-box fit).

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

const SAMPLE: &str = "docx-pagination";

/// A Word section: page size, margins (top, right, bottom, left), and its
/// paragraphs as `(text, keep_next)`.
pub struct Section {
    pub width_pt: f32,
    pub height_pt: f32,
    pub margins: [f32; 4],
    pub paragraphs: Vec<(String, bool)>,
}

/// The two sections of plugin-doc's `pagination_docx()`.
pub fn sections() -> Vec<Section> {
    vec![
        Section {
            // US Letter, 1 in margins (twips 12240 x 15840, 1440).
            width_pt: 612.0,
            height_pt: 792.0,
            margins: [72.0, 72.0, 72.0, 72.0],
            paragraphs: (1..=120)
                .map(|n| (format!("S1 P{n:03} of the first section."), n == 54))
                .collect(),
        },
        Section {
            // A5 landscape, 0.5 in margins (twips 11906 x 8391, 720).
            width_pt: 595.3,
            height_pt: 419.55,
            margins: [36.0, 36.0, 36.0, 36.0],
            paragraphs: (1..=40)
                .map(|n| (format!("S2 P{n:03} of the second section."), false))
                .collect(),
        },
    ]
}

/// The body story id of section `i` (0-based).
pub fn section_story_id(i: u32) -> String {
    self_id(SAMPLE, "SectionStory", i)
}

fn paragraph(text: &str, keep_next: bool) -> Paragraph {
    let mut attrs: Vec<(&'static str, &'static str)> = vec![("Hyphenation", "false")];
    if keep_next {
        // Word's keepNext: keep with the FIRST line of the next paragraph.
        attrs.push(("KeepWithNext", "1"));
    }
    Paragraph {
        extra_paragraph_attrs: attrs,
        // Word's `w:spacing w:line="240" w:lineRule="exact"`.
        leading: Some(12.0),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(10.0),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some("Inter"),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut master_spreads = Vec::new();
    let mut spreads = Vec::new();
    let mut stories = Vec::new();
    let mut master_refs = Vec::new();
    let mut spread_refs = Vec::new();
    let mut story_refs = Vec::new();

    for (i, section) in sections().into_iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let story_id = section_story_id(seq);
        let [top, right, bottom, left] = section.margins;

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", seq),
                page_width_pt: section.width_pt,
                page_height_pt: section.height_pt,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        stories.push((
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: section
                    .paragraphs
                    .iter()
                    .map(|(t, k)| paragraph(t, *k))
                    .collect(),
            }),
        ));
        story_refs.push(story_id.clone());

        let frame: PageItem = Rect {
            self_id: self_id(SAMPLE, "SectionFrame", seq),
            width_pt: section.width_pt - left - right,
            height_pt: section.height_pt - top - bottom,
            item_transform: translate(left, top),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
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
            // Word fits a line only when its whole line box fits: the engine
            // says that as a first baseline one full leading down.
            text_frame_pref: Some(TextFramePref {
                inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
                first_baseline_offset: Some("LeadingOffset"),
                ..Default::default()
            }),
            custom_subpaths: None,
        }
        .into();
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: (i + 1).to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: section.width_pt,
                page_height_pt: section.height_pt,
                page_items: vec![frame],
                override_list: Vec::new(),
                margins: Some(crate::builders::spread::MarginPreference {
                    top,
                    bottom,
                    left,
                    right,
                    column_count: 1,
                    column_gutter: 12.0,
                }),
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
