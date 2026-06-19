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

//! W2.2 mega-file: `links-ok.idml` — the all-healthy links control.
//!
//! `links-broken.idml` deliberately mixes broken + low-res rows, so the
//! Links panel's "NO missing/lo-res badge ANYWHERE" assertion (the
//! positive AC-LINKS-3 case) can't hold there. This sample is the
//! dedicated clean control: two rectangles, each hosting an
//! inline-embedded PNG that resolves "ok" with a healthy effective PPI
//! (>= the 150-ppi preflight floor), so every Links-panel row reads ok —
//! no `missing`, no `lo-res` badge in the whole list.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, PlacedImage, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "links-ok";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 18.0;
const BODY_FONT: &str = "Inter";

/// A deterministic 2×2 RGBA PNG (solid green), inlined so the frame
/// resolves "ok" without any external asset. Same payload the
/// links-broken healthy control uses.
const GREEN_2X2_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x50, 0x3a, 0x1a, 0xf7,
    0x1f, 0x84, 0x19, 0x60, 0x0c, 0x00, 0x4d, 0x42, 0x09, 0x11, 0x4f, 0x30, 0xb7, 0xb3, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

struct Variant {
    name: &'static str,
    color_space: &'static str,
    /// Healthy effective PPI — both at/above the 150-ppi floor so no
    /// lo-res badge appears.
    effective_ppi: (f32, f32),
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "links · ok · rgb-embedded",
            color_space: "$ID/RGB",
            effective_ppi: (300.0, 300.0),
        },
        Variant {
            name: "links · ok · cmyk-embedded",
            color_space: "$ID/CMYK",
            effective_ppi: (220.0, 220.0),
        },
    ]
}

fn label_paragraph(text: &str) -> Paragraph {
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
            point_size: Some(11.0),
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

fn label(story_id: &str, frame_id: String, text: &str, y_pt: f32) -> (Rect, Story) {
    let story = Story {
        extra_story_attrs: Vec::new(),
        self_id: story_id.to_string(),
        paragraphs: vec![label_paragraph(text)],
    };
    let frame = Rect {
        self_id: frame_id,
        width_pt: LABEL_W_PT,
        height_pt: LABEL_H_PT,
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
    };
    (frame, story)
}

pub fn build() -> Sample {
    let variants = variants();

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
    let mut items: Vec<PageItem> = Vec::new();

    for (i, v) in variants.iter().enumerate() {
        let seq = i as u32;
        let label_story_id = self_id(SAMPLE, "Story", seq);
        let label_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let rect_id = self_id(SAMPLE, "Rectangle", seq);
        let image_id = self_id(SAMPLE, "Image", seq);

        let row_top = 36.0 + (seq as f32) * 200.0;
        let (label_frame, label_story) = label(&label_story_id, label_frame_id, v.name, row_top);
        stories.push((label_story_id.clone(), write_story(&label_story)));
        story_refs.push(label_story_id);
        items.push(label_frame.into());

        let placed = PlacedImage {
            link_resource_uri: "file:embedded-ok.png".to_string(),
            fitting: "FitContentToFrame",
            left_crop: 0.0,
            top_crop: 0.0,
            right_crop: 0.0,
            bottom_crop: 0.0,
            image_self_id: image_id,
            image_w_pt: 2.0,
            image_h_pt: 2.0,
            image_item_transform: None,
            effective_ppi: Some(v.effective_ppi),
            actual_ppi: Some((300.0, 300.0)),
            color_space: Some(v.color_space),
            inline_bytes: Some(GREEN_2X2_PNG.to_vec()),
            clipping_path: None,
        };

        let rect = Rect {
            self_id: rect_id,
            width_pt: 200.0,
            height_pt: 150.0,
            item_transform: translate(36.0, row_top + 20.0),
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
        items.push(rect.into());
    }

    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: page_id,
        page_name: "links · all-ok".to_string(),
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
