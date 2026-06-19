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

//! W4.11 mega-file: `nested-groups.idml` — group-of-groups (W1.20).
//!
//! `geometry-groups.idml` already covers single-rect-deep nesting
//! (translate ∘ rotate ∘ scale, counter-rotation). This fixture is the
//! complementary "group-of-groups" surface W1.20 names: an OUTER group
//! whose direct members are themselves GROUPS, each holding MULTIPLE
//! leaf rects, with non-trivial `ItemTransform`s on BOTH the outer and
//! every inner group. The composition the renderer must get right is
//! `outer ∘ inner ∘ leaf` over a whole sub-tree, not just a single
//! chain.
//!
//! Two A4 pages:
//!
//!   1. `nested-groups · group-of-groups · outer-translate-inner-rotate`
//!      — the outer group translates; inner group A rotates +20°, inner
//!      group B rotates −20°; each inner group holds two filled rects.
//!      A renderer that composes parent×child correctly lands the four
//!      leaves at four distinct rotated positions; a child×parent bug
//!      mislocates them.
//!   2. `nested-groups · scaled-outer · uniform-1p5`
//!      — the outer group scales 1.5×, each inner group translates to a
//!      different quadrant, leaves are axis-aligned. The leaf fill
//!      rects' baked width/height must therefore read 1.5× their
//!      authored size (the outer scale flows through both inner groups
//!      onto every leaf) — the distinctive, exactly-assertable effect.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Group, PageItem, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{compose, rotate_deg, scale, translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "nested-groups";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const LEAF_W_PT: f32 = 60.0;
const LEAF_H_PT: f32 = 40.0;

/// A filled leaf rectangle at a local `item_transform`.
fn leaf(self_id: String, item_transform: Matrix, color: &str) -> PageItem {
    Rect {
        self_id,
        width_pt: LEAF_W_PT,
        height_pt: LEAF_H_PT,
        item_transform,
        fill_color: Some(color.to_string()),
        stroke_color: Some("Color/Black".to_string()),
        stroke_weight_pt: Some(0.5),
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
    .into()
}

/// Build the variant-specific group-of-groups for page `idx`.
fn build_variant(seq: u32, idx: usize) -> Vec<PageItem> {
    let outer_id = self_id(SAMPLE, "OuterGroup", seq);
    let inner_a_id = self_id(SAMPLE, "InnerGroupA", seq);
    let inner_b_id = self_id(SAMPLE, "InnerGroupB", seq);
    let a0 = self_id(SAMPLE, "LeafA0", seq);
    let a1 = self_id(SAMPLE, "LeafA1", seq);
    let b0 = self_id(SAMPLE, "LeafB0", seq);
    let b1 = self_id(SAMPLE, "LeafB1", seq);

    match idx {
        // 1. Outer translate; inner A rotates +20°, inner B rotates −20°.
        //    Each inner group holds two leaves side-by-side.
        0 => {
            let inner_a = Group {
                self_id: inner_a_id,
                item_transform: rotate_deg(20.0),
                children: vec![
                    leaf(a0, IDENTITY, "Color/RGBCyan"),
                    leaf(a1, translate(LEAF_W_PT + 16.0, 0.0), "Color/RGBMagenta"),
                ],
            };
            let inner_b = Group {
                self_id: inner_b_id,
                item_transform: compose(rotate_deg(-20.0), translate(0.0, 160.0)),
                children: vec![
                    leaf(b0, IDENTITY, "Color/RGBCyan"),
                    leaf(b1, translate(LEAF_W_PT + 16.0, 0.0), "Color/RGBMagenta"),
                ],
            };
            let outer = Group {
                self_id: outer_id,
                item_transform: translate(PAGE_W_PT * 0.30, PAGE_H_PT * 0.30),
                children: vec![inner_a.into(), inner_b.into()],
            };
            vec![outer.into()]
        }
        // 2. Outer scales 1.5×; inner groups translate to two quadrants;
        //    leaves axis-aligned. The outer scale composes onto every
        //    leaf, so each leaf fill reads 1.5× LEAF_W × LEAF_H.
        1 => {
            let inner_a = Group {
                self_id: inner_a_id,
                item_transform: translate(0.0, 0.0),
                children: vec![
                    leaf(a0, IDENTITY, "Color/RGBCyan"),
                    leaf(a1, translate(0.0, LEAF_H_PT + 12.0), "Color/RGBMagenta"),
                ],
            };
            let inner_b = Group {
                self_id: inner_b_id,
                item_transform: translate(LEAF_W_PT + 24.0, 0.0),
                children: vec![
                    leaf(b0, IDENTITY, "Color/RGBCyan"),
                    leaf(b1, translate(0.0, LEAF_H_PT + 12.0), "Color/RGBMagenta"),
                ],
            };
            let outer = Group {
                self_id: outer_id,
                item_transform: compose(
                    scale(1.5, 1.5),
                    translate(PAGE_W_PT * 0.25, PAGE_H_PT * 0.30),
                ),
                children: vec![inner_a.into(), inner_b.into()],
            };
            vec![outer.into()]
        }
        _ => Vec::new(),
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let names: Vec<&'static str> = vec![
        "nested-groups · group-of-groups · outer-translate-inner-rotate",
        "nested-groups · scaled-outer · uniform-1p5",
    ];

    let mut master_spreads = Vec::with_capacity(names.len());
    let mut spreads = Vec::with_capacity(names.len());
    let mut stories = Vec::with_capacity(names.len());
    let mut master_refs = Vec::with_capacity(names.len());
    let mut spread_refs = Vec::with_capacity(names.len());
    let mut story_refs = Vec::with_capacity(names.len());

    for (i, name) in names.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let label_frame_id = self_id(SAMPLE, "TextFrame", seq);

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
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain(*name)],
            }),
        ));
        story_refs.push(story_id.clone());

        let label = Rect {
            self_id: label_frame_id,
            width_pt: 460.0,
            height_pt: 24.0,
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

        let mut page_items: Vec<PageItem> = vec![label.into()];
        page_items.extend(build_variant(seq, i));

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items,
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
        graphic_xml: graphic_xml_with_extras(&extra_colors()),
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

fn extra_colors() -> Vec<ExtraColor> {
    vec![
        ExtraColor {
            self_id: "Color/RGBCyan".to_string(),
            name: "RGB Cyan".to_string(),
            space: "RGB",
            value: "0 200 220".to_string(),
        },
        ExtraColor {
            self_id: "Color/RGBMagenta".to_string(),
            name: "RGB Magenta".to_string(),
            space: "RGB",
            value: "220 0 160".to_string(),
        },
    ]
}
