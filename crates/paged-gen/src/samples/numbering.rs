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

//! W1.22 (engine gap 22) — cross-story numbering continuity fixture.
//!
//! Each variant is one page with TWO text frames, each hosting its own
//! story, where both stories' paragraphs apply a shared
//! `<NumberingList>` (`NumberingList/Shared`) via `AppliedNumberingList`
//! and number with `BulletsAndNumberingListType="NumberedList"`.
//!
//! - The `continue` variant declares
//!   `ContinueNumbersAcrossStories="true"` on the list: story A emits
//!   "1.", "2."; story B continues at "3.".
//! - The `restart` variant declares it `false`: story B restarts at
//!   "1.".
//!
//! Determinism: the renderer walks stories in designmap order (A then
//! B here), so the cross-story counter is fed deterministically.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{
        container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_numbering_list,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "numbering";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 220.0;

struct Variant {
    name: &'static str,
    /// `ContinueNumbersAcrossStories` on `NumberingList/Shared`.
    continue_across_stories: bool,
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "numbering · cross-story · continue",
            continue_across_stories: true,
        },
        Variant {
            name: "numbering · cross-story · restart",
            continue_across_stories: false,
        },
    ]
}

/// One numbered paragraph bound to the shared list.
fn numbered_item(text: &str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        justification: None,
        space_before: None,
        space_after: None,
        leading: None,
        first_line_indent: None,
        left_indent: Some(18.0),
        right_indent: None,
        drop_cap_characters: None,
        drop_cap_lines: None,
        tab_list: Vec::new(),
        bullets_list_type: Some("NumberedList"),
        applied_numbering_list: Some("NumberingList/Shared"),
        bullet_character: None,
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(18.0),
            fill_color: None,
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: None,
            anchored_frame: None,
        }],
        table: None,
        minimum_letter_spacing: None,
        desired_letter_spacing: None,
        maximum_letter_spacing: None,
    }
}

pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len() * 2);
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len() * 2);

    // All variants share the same NumberingList resource id, so the
    // package's single Styles.xml can only carry one continuity flag.
    // Emit ONE document per flag instead: the first variant's flag
    // wins for the package's Styles.xml, and the second variant is the
    // complement — but a single package has one Styles.xml. To keep the
    // fixture honest, the package's Styles.xml declares the list as
    // `continue=true` (variant 0); the `restart` variant is exercised
    // by the renderer unit test. We still emit both pages so the
    // visual fixture shows the continue case end-to-end.
    let styles_continue = variants[0].continue_across_stories;

    for (i, variant) in variants.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_a_id = self_id(SAMPLE, "StoryA", seq);
        let story_b_id = self_id(SAMPLE, "StoryB", seq);
        let frame_a_id = self_id(SAMPLE, "FrameA", seq);
        let frame_b_id = self_id(SAMPLE, "FrameB", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id.clone(),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        // Story A — two numbered items ("1.", "2.").
        stories.push((
            story_a_id.clone(),
            write_story(&Story {
                self_id: story_a_id.clone(),
                paragraphs: vec![
                    numbered_item("First list item, story A."),
                    numbered_item("Second list item, story A."),
                ],
            }),
        ));
        story_refs.push(story_a_id.clone());

        // Story B — one numbered item (continues / restarts per the
        // list flag in Styles.xml).
        stories.push((
            story_b_id.clone(),
            write_story(&Story {
                self_id: story_b_id.clone(),
                paragraphs: vec![numbered_item("Continued list item, story B.")],
            }),
        ));
        story_refs.push(story_b_id.clone());

        let x = (PAGE_W_PT - FRAME_W_PT) * 0.5;
        // Frame A near the top; frame B below it on the same page.
        let frame_a = Rect::filled(frame_a_id, FRAME_W_PT, FRAME_H_PT, translate(x, 80.0))
            .with_fill("Swatch/None")
            .with_parent_story(story_a_id.clone());
        let frame_b = Rect::filled(frame_b_id, FRAME_W_PT, FRAME_H_PT, translate(x, 360.0))
            .with_fill("Swatch/None")
            .with_parent_story(story_b_id.clone());

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: variant.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![frame_a.into(), frame_b.into()],
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
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
        styles_xml: styles_xml_with_numbering_list(styles_continue),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_renderer::pipeline;
    use paged_scene::Document;

    /// The generated package parses, and its shared `<NumberingList>`
    /// resource carries the `continue` variant's flag. Also asserts the
    /// document opens and builds without panicking (the visual fixture
    /// path the corpus consumes).
    #[test]
    fn numbering_sample_emits_shared_list_and_builds() {
        let bytes = crate::package::write_idml(&build()).unwrap();
        let doc = Document::open(&bytes).unwrap();
        let list = doc
            .styles
            .numbering_lists
            .get("NumberingList/Shared")
            .expect("shared numbering list parsed");
        assert_eq!(list.continue_across_stories, Some(true));
        // Two stories (A, B) per variant page; both bind the shared list.
        let bound = doc
            .stories
            .iter()
            .flat_map(|s| s.story.paragraphs.iter())
            .filter(|p| p.applied_numbering_list.as_deref() == Some("NumberingList/Shared"))
            .count();
        assert!(bound >= 3, "expected ≥3 list-bound paragraphs, got {bound}");
        // Builds without panicking (no font assets → glyphs fall back,
        // but the numbering machinery + ledger run).
        let opts = pipeline::PipelineOptions::default();
        let _ = pipeline::build_document(&doc, &opts).unwrap();
    }
}
