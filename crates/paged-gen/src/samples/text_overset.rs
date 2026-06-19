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

//! Aftercare-D mega-file: `text-overset.idml`.
//!
//! Two A4 pages that deterministically drive the renderer's
//! **overset** diagnostic (`DiagnosticCode::OversetTextDropped`), which
//! the editor surfaces as `StorySummary.overset` and the "N overset
//! stories" preflight badge (panels.md gap 1):
//!
//!   * **page 1 — `overset · short-frame`**: a single text frame far too
//!     short to hold its story. The story is many lines of body copy;
//!     the frame is ~40 pt tall, so most lines fall past its bottom edge
//!     and are dropped. One story, one frame, decisively overset.
//!   * **page 2 — `overset · threaded-chain`**: two short frames
//!     threaded into one chain (frame A `NextTextFrame` → frame B,
//!     frame B `PreviousTextFrame` → frame A) sharing one `ParentStory`.
//!     The story overflows *both* frames combined, so the chain itself
//!     is overset — the case the threading-port / continued-frame badge
//!     tests need.
//!
//! Body runs pin `AppliedFont="Inter"` so shaping is deterministic
//! against the harness-registered `corpus/fonts/Inter.ttf` (the same
//! font the canvas overset test loads). Overset is a *layout-time*
//! property: it only fires once the renderer shapes the text and finds
//! lines past the last frame, so the sample test builds the document
//! through `paged-renderer` to assert the diagnostic actually fires
//! rather than asserting a structural proxy.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text-overset";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const LABEL_W_PT: f32 = 480.0;
const LABEL_H_PT: f32 = 24.0;

/// Body font family pinned on every run so shaping resolves to the
/// harness-registered Inter face deterministically.
const BODY_FONT: &str = "Inter";
const BODY_PT: f32 = 12.0;

/// A single short frame holds maybe 2-3 lines at 12 pt; the story below
/// is far longer, so the bulk of it is overset.
const SHORT_FRAME_W_PT: f32 = 360.0;
const SHORT_FRAME_H_PT: f32 = 40.0;

/// Body copy long enough to overflow a 40 pt frame several times over.
/// Each entry is its own paragraph so the line count is unambiguous.
fn body_paragraphs() -> Vec<&'static str> {
    vec![
        "The quick brown fox jumps over the lazy dog.",
        "Pack my box with five dozen liquor jugs.",
        "Sphinx of black quartz, judge my vow.",
        "How vexingly quick daft zebras jump!",
        "Bright vixens jump; dozy fowl quack.",
        "Jackdaws love my big sphinx of quartz.",
        "The five boxing wizards jump quickly.",
        "Waltz, bad nymph, for quick jigs vex.",
        "Glib jocks quiz nymph to vex dwarf.",
        "Crazy Frederick bought many very exquisite opal jewels.",
    ]
}

/// One body paragraph pinned to Inter at `BODY_PT`.
fn inter_paragraph(text: &str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        justification: None,
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
        minimum_letter_spacing: None,
        desired_letter_spacing: None,
        maximum_letter_spacing: None,
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(BODY_PT),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
    }
}

/// A label text frame + its backing story. Returns `(frame, story)`.
fn label(page_name: &str, story_id: &str, frame_id: String) -> (Rect, Story) {
    let story = Story {
        extra_story_attrs: Vec::new(),
        self_id: story_id.to_string(),
        paragraphs: vec![inter_paragraph_label(page_name)],
    };
    let frame = Rect {
        self_id: frame_id,
        width_pt: LABEL_W_PT,
        height_pt: LABEL_H_PT,
        item_transform: translate(36.0, 36.0),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story_id.to_string()),
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
    (frame, story)
}

/// Label paragraph (Inter, slightly larger than body so the descriptor
/// reads as a heading on the page).
fn inter_paragraph_label(text: &str) -> Paragraph {
    let mut p = inter_paragraph(text);
    p.runs[0].point_size = Some(14.0);
    p
}

pub fn build() -> Sample {
    // Resource/master scaffolding shared by both pages.
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });

    let mut spreads: Vec<(String, Vec<u8>)> = Vec::new();
    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut spread_refs: Vec<String> = Vec::new();
    let mut story_refs: Vec<String> = Vec::new();

    // ── Page 1 — single short frame, overset ─────────────────────
    {
        let spread_id = self_id(SAMPLE, "Spread", 0);
        let page_id = self_id(SAMPLE, "Page", 0);
        let label_story_id = self_id(SAMPLE, "Story", 0);
        let label_frame_id = self_id(SAMPLE, "TextFrame", 0);
        let body_story_id = self_id(SAMPLE, "Story", 1);
        let body_frame_id = self_id(SAMPLE, "TextFrame", 1);

        let (label_frame, label_story) =
            label("overset · short-frame", &label_story_id, label_frame_id);
        stories.push((label_story_id.clone(), write_story(&label_story)));
        story_refs.push(label_story_id);

        // The overset story: 10 paragraphs of body copy in a 40 pt frame.
        let body_story = Story {
            extra_story_attrs: Vec::new(),
            self_id: body_story_id.clone(),
            paragraphs: body_paragraphs().into_iter().map(inter_paragraph).collect(),
        };
        stories.push((body_story_id.clone(), write_story(&body_story)));
        story_refs.push(body_story_id.clone());

        let body_frame = Rect {
            self_id: body_frame_id,
            width_pt: SHORT_FRAME_W_PT,
            height_pt: SHORT_FRAME_H_PT,
            item_transform: translate(36.0, 96.0),
            fill_color: None,
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
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

        let items: Vec<PageItem> = vec![label_frame.into(), body_frame.into()];
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: "overset · short-frame".to_string(),
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

    // ── Page 2 — threaded two-frame chain, also overset ──────────
    {
        let spread_id = self_id(SAMPLE, "Spread", 1);
        let page_id = self_id(SAMPLE, "Page", 1);
        let label_story_id = self_id(SAMPLE, "Story", 2);
        let label_frame_id = self_id(SAMPLE, "TextFrame", 2);
        let chain_story_id = self_id(SAMPLE, "Story", 3);
        let frame_a_id = self_id(SAMPLE, "TextFrame", 3);
        let frame_b_id = self_id(SAMPLE, "TextFrame", 4);

        let (label_frame, label_story) =
            label("overset · threaded-chain", &label_story_id, label_frame_id);
        stories.push((label_story_id.clone(), write_story(&label_story)));
        story_refs.push(label_story_id);

        // Same long body — split across the A→B chain. Two 40 pt frames
        // together hold ~4-6 lines; the story is 10 paragraphs, so the
        // chain still oversets past frame B.
        let chain_story = Story {
            extra_story_attrs: Vec::new(),
            self_id: chain_story_id.clone(),
            paragraphs: body_paragraphs().into_iter().map(inter_paragraph).collect(),
        };
        stories.push((chain_story_id.clone(), write_story(&chain_story)));
        story_refs.push(chain_story_id.clone());

        // Frame A — head of the chain: ParentStory = chain story,
        // NextTextFrame → B. No PreviousTextFrame (it's the head).
        let frame_a = Rect {
            self_id: frame_a_id.clone(),
            width_pt: SHORT_FRAME_W_PT,
            height_pt: SHORT_FRAME_H_PT,
            item_transform: translate(36.0, 96.0),
            fill_color: None,
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
            parent_story: Some(chain_story_id.clone()),
            next_text_frame: Some(frame_b_id.clone()),
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
        // Frame B — tail of the chain: same ParentStory,
        // PreviousTextFrame → A, no NextTextFrame (end of chain, so
        // overflow past B is dropped → overset).
        let frame_b = Rect {
            self_id: frame_b_id,
            width_pt: SHORT_FRAME_W_PT,
            height_pt: SHORT_FRAME_H_PT,
            item_transform: translate(36.0, 180.0),
            fill_color: None,
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
            parent_story: Some(chain_story_id),
            next_text_frame: None,
            previous_text_frame: Some(frame_a_id),
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

        let items: Vec<PageItem> = vec![label_frame.into(), frame_a.into(), frame_b.into()];
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: "overset · threaded-chain".to_string(),
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

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
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
        master_spreads: vec![(master_id, master)],
        spreads,
        stories,
    }
}
