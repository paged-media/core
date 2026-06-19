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

//! W2.2 mega-file: `preflight.idml` — the "unhealthy publication".
//!
//! One A4 page whose content deliberately stacks the publication-health
//! signals the W2.12 panels surface, closing the honest gap that no
//! generated fixture carried a by-design missing font:
//!
//!   * **overset body**: a short text frame holding a long story, so the
//!     layout drops the overflow and flags the story overset
//!     (`StorySummary.overset` → the publication-health "overset" tile).
//!   * **missing font**: a run pinned to `AppliedFont="Phantom Display"`,
//!     a family no corpus font (and no harness registration) provides, so
//!     `FontSummary.is_missing` is true BY DESIGN — independent of the
//!     runner's font set (unlike the incidental "Open Sans" substitution
//!     in `text.idml`). This is the Fonts panel's deterministic missing
//!     case (AC-FONTS-3).
//!   * **undecodable placed image**: a `<Rectangle>` hosting an `<Image>`
//!     whose inline `<Contents>` payload is NOT a valid image (a short
//!     non-image byte string) — a genuinely broken placement.
//!
//! IMPORTANT — preflight findings vs build placeholders. The PDF
//! exporter only emits a `PreflightFinding` for `font_not_embeddable`
//! (an fsType-locked font; the OFL corpus fonts never trip it) and
//! `image_missing_bytes` (an `<Image>` command that survives to the
//! exporter with undecodable bytes). The undecodable image above does
//! NOT reach the exporter: `paged-renderer` decodes inline `<Contents>`
//! at BUILD time, fails, and stamps the missing-image placeholder, so
//! the display list carries no `<Image>` command for it (verified
//! against the 0.35.1 wasm — `exportPdf` returns zero findings here).
//! The overset + missing-link signals are therefore build-time /
//! model-level only; surfacing them as export-time `PreflightFinding`s
//! is an engine gap (see the editor AC-PREFLIGHT-2 fixme). This sample
//! flips AC-FONTS-3 today and documents the rest as the punch list.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PlacedImage, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "preflight";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 18.0;
const BODY_FONT: &str = "Inter";

/// The by-design missing family — no corpus font provides it and the
/// harness never registers it, so `FontSummary.is_missing` is stable.
pub const MISSING_FAMILY: &str = "Phantom Display";

/// Deliberately-undecodable "image" bytes for the export-finding case:
/// present (so the placement reaches the exporter) but not a valid PNG /
/// JPEG, so `image::load_from_memory` fails at export → the
/// `image_missing_bytes` preflight finding fires.
const CORRUPT_IMAGE_BYTES: &[u8] = b"NOT-A-REAL-IMAGE-PAYLOAD-0123456789";

/// A long body paragraph (overset in the short frame below).
fn overset_body() -> Paragraph {
    let long = "This story is far longer than its frame can hold, so the \
        composer drops the overflow lines and flags the story overset. \
        The publication-health panel surfaces that as the overset count, \
        and the preflight gate counts it among the document's issues. \
        Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do \
        eiusmod tempor incididunt ut labore et dolore magna aliqua.";
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        runs: vec![Run {
            text: long.to_string(),
            point_size: Some(14.0),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
        ..plain_paragraph()
    }
}

/// A short paragraph pinned to the missing family.
fn missing_font_body() -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        runs: vec![Run {
            text: "Set in a font this document declares but no host provides.".to_string(),
            point_size: Some(16.0),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(MISSING_FAMILY),
            anchored_frame: None,
        }],
        ..plain_paragraph()
    }
}

fn label_paragraph(text: &str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        runs: vec![Run {
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
        ..plain_paragraph()
    }
}

/// A Paragraph with every optional field cleared (runs filled by the
/// caller via struct-update).
fn plain_paragraph() -> Paragraph {
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
        runs: Vec::new(),
    }
}

/// A label text frame + its backing story.
fn label_frame(story_id: &str, frame_id: String, text: &str, y_pt: f32) -> (Rect, Story) {
    let story = Story {
        self_id: story_id.to_string(),
        paragraphs: vec![label_paragraph(text)],
    };
    let frame = text_frame(frame_id, story_id, LABEL_W_PT, LABEL_H_PT, y_pt);
    (frame, story)
}

fn text_frame(frame_id: String, story_id: &str, w: f32, h: f32, y_pt: f32) -> Rect {
    Rect {
        self_id: frame_id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(36.0, y_pt),
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
    }
}

pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });

    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut story_refs: Vec<String> = Vec::new();
    let mut items: Vec<crate::builders::page_item::PageItem> = Vec::new();

    // ── 1. overset body ──────────────────────────────────────────
    let overset_story_id = self_id(SAMPLE, "Story", 0);
    let overset_frame_id = self_id(SAMPLE, "TextFrame", 0);
    stories.push((
        overset_story_id.clone(),
        write_story(&Story {
            self_id: overset_story_id.clone(),
            paragraphs: vec![overset_body()],
        }),
    ));
    story_refs.push(overset_story_id.clone());
    // A short 40pt frame far too small for the long story → overset.
    items.push(text_frame(overset_frame_id, &overset_story_id, 360.0, 40.0, 36.0).into());

    // ── 2. missing-font body ─────────────────────────────────────
    let mf_story_id = self_id(SAMPLE, "Story", 1);
    let mf_frame_id = self_id(SAMPLE, "TextFrame", 1);
    stories.push((
        mf_story_id.clone(),
        write_story(&Story {
            self_id: mf_story_id.clone(),
            paragraphs: vec![missing_font_body()],
        }),
    ));
    story_refs.push(mf_story_id.clone());
    items.push(text_frame(mf_frame_id, &mf_story_id, 480.0, 60.0, 120.0).into());

    // ── 3. undecodable placed image ──────────────────────────────
    let label_story_id = self_id(SAMPLE, "Story", 2);
    let label_frame_id = self_id(SAMPLE, "TextFrame", 2);
    let (lbl, lbl_story) = label_frame(
        &label_story_id,
        label_frame_id,
        "preflight · image · undecodable",
        200.0,
    );
    stories.push((label_story_id.clone(), write_story(&lbl_story)));
    story_refs.push(label_story_id);
    items.push(lbl.into());

    let rect_id = self_id(SAMPLE, "Rectangle", 0);
    let image_id = self_id(SAMPLE, "Image", 0);
    let placed = PlacedImage {
        link_resource_uri: "file:embedded-broken.png".to_string(),
        fitting: "FitContentToFrame",
        left_crop: 0.0,
        top_crop: 0.0,
        right_crop: 0.0,
        bottom_crop: 0.0,
        image_self_id: image_id,
        image_w_pt: 2.0,
        image_h_pt: 2.0,
        image_item_transform: None,
        effective_ppi: Some((72.0, 72.0)),
        actual_ppi: Some((72.0, 72.0)),
        color_space: Some("$ID/RGB"),
        // Present but undecodable → reaches the exporter, decode fails.
        inline_bytes: Some(CORRUPT_IMAGE_BYTES.to_vec()),
        clipping_path: None,
    };
    let image_rect = Rect {
        self_id: rect_id,
        width_pt: 200.0,
        height_pt: 150.0,
        item_transform: translate(36.0, 224.0),
        fill_color: Some("Color/Paper".to_string()),
        stroke_color: Some("Color/Black".to_string()),
        stroke_weight_pt: Some(0.5),
        parent_story: None,
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: Some(placed),
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    };
    items.push(image_rect.into());

    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: page_id,
        page_name: "preflight · overset+missing-font+image".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: items,
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: vec![spread_id.clone()],
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
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
