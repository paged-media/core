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

//! W1.21 mega-file: `image-clipping.idml`.
//!
//! A placed image carrying a detached `<ClippingPathSettings>` clip
//! (InDesign clips the picture to a path *in addition to* the frame
//! outline). Each page embeds the same 100×100 RGBA PNG fixture inline
//! (no external asset needed) inside a 100×100 pt frame with the inner
//! `<Image ItemTransform>` mapping 1 image-pixel → 1 pt, so the clip
//! anchors (authored in 0..100 image-pixel space) land 1:1 on the
//! frame. Variants exercise:
//!
//!   * `UserModifiedPath` star — the resolved geometry rides along as a
//!     `<PathGeometry>`; the image is clipped to the star.
//!   * `UserModifiedPath` star-with-hole — a compound clip (outer star +
//!     inner punched diamond) with `IncludeInsideEdges="true"`; the hole
//!     survives via `subpath_starts`.
//!   * `UserModifiedPath` + `InvertPath="true"` — a rectangle clip kept
//!     *outside* the path (the rectangle becomes a hole in the picture).
//!   * `PhotoshopPath` (deferred) — references a named 8BIM path with no
//!     inline geometry; the renderer records a diagnostic and renders the
//!     image clipped to the frame only.

use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml,
};
use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{ClipPathSpec, PlacedImage, PolygonSubPath, Rect},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "image-clipping";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const FRAME_PT: f32 = 100.0;
const LABEL_W_PT: f32 = 420.0;
const LABEL_H_PT: f32 = 24.0;
const LINK_URI: &str = "file:clip-fixture.png";

/// A 100×100 RGBA PNG (left half teal, right half orange). Inlined as
/// base64 `<Contents>` so the sample resolves "ok" with no resolver —
/// the clip is the visible feature, not the picture content. 100 px so
/// clip anchors authored in 0..100 pixel space are meaningful.
const CLIP_FIXTURE_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00, 0x64, 0x08, 0x06, 0x00, 0x00, 0x00, 0x70, 0xe2, 0x95,
    0x54, 0x00, 0x00, 0x00, 0xd2, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0xed, 0xd1, 0x31, 0x11, 0x00,
    0x30, 0x08, 0x04, 0x30, 0x14, 0x21, 0x10, 0x41, 0xe8, 0xaa, 0x93, 0xe2, 0xe3, 0x2f, 0x43, 0x14,
    0xa4, 0x7a, 0xf7, 0x27, 0x78, 0xd3, 0x11, 0x4a, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22,
    0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08,
    0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42,
    0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10,
    0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44,
    0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11,
    0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84,
    0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21,
    0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88,
    0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22,
    0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08, 0x11, 0x22, 0x44, 0x88, 0x10, 0x21, 0x42, 0x84, 0x08,
    0x11, 0x22, 0x44, 0x48, 0x7a, 0xc8, 0x01, 0x45, 0x6f, 0xad, 0xb0, 0xb1, 0xb5, 0x9c, 0x8a, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A 5-point star in 0..100 image-pixel space, centred at (50, 50).
/// Outer radius 48, inner radius 20. Straight corners.
fn star_points(cx: f32, cy: f32, r_out: f32, r_in: f32) -> Vec<(f32, f32)> {
    let mut pts = Vec::with_capacity(10);
    // Start at the top (−90°) and alternate outer/inner every 36°.
    for i in 0..10 {
        let r = if i % 2 == 0 { r_out } else { r_in };
        let ang = (-90.0_f32 + (i as f32) * 36.0).to_radians();
        pts.push((cx + r * ang.cos(), cy + r * ang.sin()));
    }
    pts
}

struct Variant {
    name: &'static str,
    clip: ClipPathSpec,
}

fn variants() -> Vec<Variant> {
    vec![
        // 1. Star clip (UserModifiedPath, single contour).
        Variant {
            name: "image-clip · star · userpath",
            clip: ClipPathSpec {
                clipping_type: "UserModifiedPath",
                invert: false,
                include_inside_edges: false,
                applied_path_name: None,
                subpaths: vec![PolygonSubPath::corners(
                    star_points(50.0, 50.0, 48.0, 20.0),
                    true,
                )],
            },
        },
        // 2. Star with a punched diamond hole (compound clip). The inner
        //    diamond is authored CW (same winding as the star here); the
        //    renderer normalises so it survives as a hole under NonZero.
        Variant {
            name: "image-clip · star-hole · include-inside-edges",
            clip: ClipPathSpec {
                clipping_type: "UserModifiedPath",
                invert: false,
                include_inside_edges: true,
                applied_path_name: None,
                subpaths: vec![
                    PolygonSubPath::corners(star_points(50.0, 50.0, 48.0, 20.0), true),
                    // Small centred diamond hole.
                    PolygonSubPath::corners(
                        [(50.0, 38.0), (62.0, 50.0), (50.0, 62.0), (38.0, 50.0)],
                        true,
                    ),
                ],
            },
        },
        // 3. Inverted rectangle clip — keep the area OUTSIDE the path, so
        //    a 40×40 rectangle in the picture centre is punched out.
        Variant {
            name: "image-clip · rect · invert",
            clip: ClipPathSpec {
                clipping_type: "UserModifiedPath",
                invert: true,
                include_inside_edges: false,
                applied_path_name: None,
                subpaths: vec![PolygonSubPath::corners(
                    [(30.0, 30.0), (70.0, 30.0), (70.0, 70.0), (30.0, 70.0)],
                    true,
                )],
            },
        },
        // 4. PhotoshopPath — named 8BIM path, no inline geometry. The
        //    renderer can't reach the path in the image binary, so it
        //    defers (diagnostic + frame-only clip).
        Variant {
            name: "image-clip · photoshop-path · deferred",
            clip: ClipPathSpec {
                clipping_type: "PhotoshopPath",
                invert: false,
                include_inside_edges: false,
                applied_path_name: Some("Path 1"),
                subpaths: Vec::new(),
            },
        },
    ]
}

pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len());

    for (i, variant) in variants.into_iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let label_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let frame_id = self_id(SAMPLE, "Rectangle", seq);
        let image_id = self_id(SAMPLE, "Image", seq);

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
            story_id.clone(),
            write_story(&Story {
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(story_id.clone());

        // Label TextFrame top-left.
        let label = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
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
            text_frame_pref: None,
            custom_subpaths: None,
        };

        // Centre the 100×100 frame on the page.
        let cx = (PAGE_W_PT - FRAME_PT) * 0.5;
        let cy = (PAGE_H_PT - FRAME_PT) * 0.5;

        let frame = Rect {
            self_id: frame_id,
            width_pt: FRAME_PT,
            height_pt: FRAME_PT,
            item_transform: translate(cx, cy),
            fill_color: Some("Color/Paper".to_string()),
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
            parent_story: None,
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: Some(PlacedImage {
                link_resource_uri: LINK_URI.to_string(),
                fitting: "FitContentToFrame",
                left_crop: 0.0,
                top_crop: 0.0,
                right_crop: 0.0,
                bottom_crop: 0.0,
                image_self_id: image_id,
                // Native pixel dims match the decoded fixture (100×100),
                // so 1 image-pixel maps to 1 pt under the identity inner
                // transform — clip anchors in 0..100 land 1:1 on the
                // 100 pt frame.
                image_w_pt: 100.0,
                image_h_pt: 100.0,
                // Identity inner transform: the image pixel rect maps
                // straight into the frame's inner coords. (`Some` selects
                // the renderer's clipping-capable placement path.)
                image_item_transform: Some(crate::geometry::IDENTITY),
                effective_ppi: None,
                actual_ppi: None,
                color_space: Some("$ID/RGB"),
                inline_bytes: Some(CLIP_FIXTURE_PNG.to_vec()),
                clipping_path: Some(variant.clip),
            }),
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
                page_items: vec![label.into(), frame.into()],
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
