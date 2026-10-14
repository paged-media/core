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

//! `placed-pdf.idml` — a placed PDF page drawn from its own operations,
//! asked of InDesign (A-20, ADR 154).
//!
//! `placed-art.pdf` (`corpus/generated/placed-pdf-Links/`, written by its
//! `make-placed-art.py`) is a 200 × 120 pt page of vector art: process
//! and RGB fills, a dashed stroke, a curve, a clipped square and a 50 %
//! black panel. It is placed four times:
//!
//! 1. at 100 %, the frame its own size;
//! 2. at 50 % in a 100 × 60 frame;
//! 3. at 100 % in a frame rotated 30°;
//! 4. at 100 % in a 120 × 80 frame that crops it, offset (−40, −20).
//!
//! The link is an absolute path InDesign can open while it exports
//! (`file:/tmp/paged-placed-art/placed-art.pdf`; the export copies the
//! asset there). The engine resolves it by its basename in the fixture's
//! `-Links` folder, as every generated link.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PdfPlacement, PlacedImage, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{compose, rotate_deg, scale, translate, Matrix};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "placed-pdf";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
pub const ART_W: f32 = 200.0;
pub const ART_H: f32 = 120.0;
pub const LINK: &str = "file:/tmp/paged-placed-art/placed-art.pdf";

struct Case {
    label: &'static str,
    frame: (f32, f32),
    at: (f32, f32),
    rotate: f32,
    /// The PDF's `ItemTransform` inside the frame.
    art: Matrix,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "1 · 100 %",
            frame: (ART_W, ART_H),
            at: (60.0, 90.0),
            rotate: 0.0,
            art: translate(0.0, 0.0),
        },
        Case {
            label: "2 · 50 %",
            frame: (ART_W * 0.5, ART_H * 0.5),
            at: (320.0, 90.0),
            rotate: 0.0,
            art: scale(0.5, 0.5),
        },
        Case {
            label: "3 · rotated 30°",
            frame: (ART_W, ART_H),
            at: (100.0, 300.0),
            rotate: 30.0,
            art: translate(0.0, 0.0),
        },
        Case {
            label: "4 · cropped",
            frame: (120.0, 80.0),
            at: (340.0, 300.0),
            rotate: 0.0,
            art: translate(-40.0, -20.0),
        },
    ]
}

fn frame(id: String, size: (f32, f32), transform: Matrix) -> Rect {
    Rect {
        self_id: id,
        width_pt: size.0,
        height_pt: size.1,
        item_transform: transform,
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: None,
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

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut items = Vec::new();
    for (i, c) in cases().into_iter().enumerate() {
        let n = i as u32;
        let story_id = self_id(SAMPLE, "LabelStory", n);
        stories.push((
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain(c.label)],
            }),
        ));
        let mut label = frame(
            self_id(SAMPLE, "LabelFrame", n),
            (200.0, 16.0),
            translate(c.at.0, c.at.1 - 24.0),
        );
        label.parent_story = Some(story_id);
        label.text_frame_pref = Some(TextFramePref {
            inset_spacing: Some([0.0; 4]),
            ..Default::default()
        });
        items.push(label.into());
        let mut placed = frame(
            self_id(SAMPLE, "Frame", n),
            c.frame,
            compose(translate(c.at.0, c.at.1), rotate_deg(c.rotate)),
        );
        placed.placed_image = Some(PlacedImage {
            link_resource_uri: LINK.to_string(),
            fitting: "None",
            left_crop: 0.0,
            top_crop: 0.0,
            right_crop: 0.0,
            bottom_crop: 0.0,
            image_self_id: self_id(SAMPLE, "Pdf", n),
            image_w_pt: ART_W,
            image_h_pt: ART_H,
            image_item_transform: Some(c.art),
            effective_ppi: None,
            actual_ppi: None,
            color_space: None,
            inline_bytes: None,
            clipping_path: None,
            pdf: Some(PdfPlacement {
                page: 1,
                crop: "CropMedia",
            }),
        });
        items.push(placed.into());
    }
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
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
        spreads: vec![(
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id,
                page_self_id: self_id(SAMPLE, "Page", 0),
                page_name: "1".to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        )],
        stories,
    }
}
