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

//! Phase-2 mega-file: `geometry-groups.idml`.
//!
//! Six A4-portrait pages exercising `<Group>` wrappers and nested
//! transforms — spec §4.16 + §4.17 of the sample-generator brief
//! call this "the highest-leverage area for renderer correctness."
//!
//! Each page wraps two coloured demo rectangles (or, for the
//! compound-path variant, a single Polygon with two sub-paths) so
//! the diff harness can attribute renderer divergence to a specific
//! transform-composition concern. The page label (`Page.Name`)
//! follows the established `geometry-groups · variant · detail`
//! convention so per-page heatmaps remain self-describing.
//!
//! Variants:
//!   1. Identity Group — both rects visible, untransformed.
//!   2. Group with translation — both rects translated together.
//!   3. Group with 30° rotation about origin.
//!   4. Three-deep nesting (translate ∘ rotate ∘ scale) — verifies
//!      parent×child composition order.
//!   5. Counter-rotation — outer Group rotates, inner child rotates
//!      back; the rendered rect should net to upright.
//!   6. Compound path (`Polygon` with two sub-paths in one
//!      `PathGeometry`) — even-odd fill cuts a square hole out of
//!      a square.
//!
//! Note: this sample only exercises *structural* group emission and
//! transform composition; richer group semantics (group-level
//! effects, knockout, isolate-blending, anchored objects) live in a
//! follow-up sample.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Group, PageItem, Polygon, PolygonSubPath, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{compose, rotate_deg, scale, translate, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "geometry-groups";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const RECT_W_PT: f32 = 90.0;
const RECT_H_PT: f32 = 60.0;
const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 24.0;

/// Anchor point near the top-left of the body area where the
/// Group's local origin lands. Picked so each variant's rendered
/// content stays comfortably inside the page even after rotation
/// and scale.
const GROUP_ANCHOR_X: f32 = 200.0;
const GROUP_ANCHOR_Y: f32 = 240.0;

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let variants: Vec<&'static str> = vec![
        "geometry-groups · identity · two-rects",
        "geometry-groups · translate · 80-60",
        "geometry-groups · rotate · 30-deg",
        "geometry-groups · nested-3-deep · translate-rotate-scale",
        "geometry-groups · counter-rotate · child-cancels-parent",
        "geometry-groups · compound-path · square-with-hole",
    ];

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len());

    for (i, name) in variants.iter().enumerate() {
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
                page_self_id: master_page_id.clone(),
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
                paragraphs: vec![Paragraph::plain(*name)],
            }),
        ));
        story_refs.push(story_id.clone());

        // Per-page descriptor frame — same layout as `geometry.idml`
        // so variant attribution lines up across mega-files.
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

/// Two extra coloured swatches so the demo rectangles can be
/// distinguished from one another (and from any underlay) when
/// transforms send them on top of each other.
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

/// Build the variant-specific page items (the Group, or the
/// compound Polygon) for the page at `idx`. The leading label frame
/// is appended by the caller.
fn build_variant(seq: u32, idx: usize) -> Vec<PageItem> {
    let group_id = self_id(SAMPLE, "Group", seq);
    let inner_group_id = self_id(SAMPLE, "InnerGroup", seq);
    let middle_group_id = self_id(SAMPLE, "MiddleGroup", seq);
    let rect_a_id = self_id(SAMPLE, "RectA", seq);
    let rect_b_id = self_id(SAMPLE, "RectB", seq);
    let polygon_id = self_id(SAMPLE, "Polygon", seq);

    match idx {
        // 1. Identity transform — two rects positioned in the
        //    Group's local frame at (0, 0) and (RECT_W + gap, 0).
        //    The Group itself sits at GROUP_ANCHOR.
        0 => {
            let children: Vec<PageItem> = vec![two_rect_pair(&rect_a_id, &rect_b_id)]
                .into_iter()
                .flatten()
                .collect();
            vec![Group {
                self_id: group_id,
                item_transform: translate(GROUP_ANCHOR_X, GROUP_ANCHOR_Y),
                children,
            }
            .into()]
        }
        // 2. Pure translation on the Group — the rect pair shifts
        //    en bloc by (80, 60) in spread coords. Children carry
        //    their original local positions.
        1 => {
            let children: Vec<PageItem> = two_rect_pair(&rect_a_id, &rect_b_id);
            vec![Group {
                self_id: group_id,
                item_transform: compose(
                    translate(80.0, 60.0),
                    translate(GROUP_ANCHOR_X, GROUP_ANCHOR_Y),
                ),
                children,
            }
            .into()]
        }
        // 3. 30° rotation around origin, then translate to the
        //    page anchor. The two rects rotate as a unit; the gap
        //    between them rotates too (so they no longer sit on a
        //    horizontal axis).
        2 => {
            let children: Vec<PageItem> = two_rect_pair(&rect_a_id, &rect_b_id);
            vec![Group {
                self_id: group_id,
                item_transform: compose(
                    rotate_deg(30.0),
                    translate(GROUP_ANCHOR_X, GROUP_ANCHOR_Y),
                ),
                children,
            }
            .into()]
        }
        // 4. Three-deep: outermost Group translates, middle Group
        //    rotates, innermost Group scales. The single demo
        //    rectangle inside the innermost group composes
        //    outer ∘ middle ∘ inner ∘ rect — a renderer that
        //    composes child×parent (instead of parent×child) lands
        //    the rect in a different page position.
        3 => {
            let inner_rect = Rect {
                self_id: rect_a_id,
                width_pt: RECT_W_PT,
                height_pt: RECT_H_PT,
                // Rect's own local transform is identity — every
                // shift comes from the surrounding Group stack.
                item_transform: IDENTITY,
                fill_color: Some("Color/RGBMagenta".to_string()),
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
            };
            let inner_group = Group {
                self_id: inner_group_id,
                // Innermost: 1.4× uniform scale.
                item_transform: scale(1.4, 1.4),
                children: vec![inner_rect.into()],
            };
            let middle_group = Group {
                self_id: middle_group_id,
                // Middle: 25° rotation.
                item_transform: rotate_deg(25.0),
                children: vec![inner_group.into()],
            };
            let outer_group = Group {
                self_id: group_id,
                // Outer: translate to the body region.
                item_transform: translate(GROUP_ANCHOR_X + 80.0, GROUP_ANCHOR_Y + 60.0),
                children: vec![middle_group.into()],
            };
            vec![outer_group.into()]
        }
        // 5. Counter-rotation — outer Group rotates +45°, child
        //    Rect rotates -45° in its own ItemTransform. Net:
        //    upright rectangle (at the page anchor).
        4 => {
            let counter = Rect {
                self_id: rect_a_id,
                width_pt: RECT_W_PT,
                height_pt: RECT_H_PT,
                item_transform: rotate_deg(-45.0),
                fill_color: Some("Color/RGBCyan".to_string()),
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
            };
            // Reference rect outside the group: same fill, no
            // counter-rotation, sitting alongside so the eye can
            // confirm the in-group rect lands upright (matching
            // the reference's orientation).
            let reference = Rect {
                self_id: rect_b_id,
                width_pt: RECT_W_PT,
                height_pt: RECT_H_PT,
                item_transform: translate(GROUP_ANCHOR_X + 200.0, GROUP_ANCHOR_Y),
                fill_color: Some("Color/RGBMagenta".to_string()),
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
            };
            let group = Group {
                self_id: group_id,
                item_transform: compose(
                    rotate_deg(45.0),
                    translate(GROUP_ANCHOR_X, GROUP_ANCHOR_Y),
                ),
                children: vec![counter.into()],
            };
            vec![group.into(), reference.into()]
        }
        // 6. Compound path — single Polygon, one PathGeometry, two
        //    sub-paths (outer 200×200 square, inner 80×80 square).
        //    Even-odd fill rule punches the inner square out as a
        //    hole. The PolygonItemTransform places the assembly
        //    near the page centre.
        5 => {
            let outer = 200.0_f32;
            let inner_inset = (outer - 80.0) * 0.5;
            let inner = inner_inset + 80.0;
            let outer_path = PolygonSubPath::corners(
                [(0.0, 0.0), (outer, 0.0), (outer, outer), (0.0, outer)],
                true,
            );
            // Inner sub-path walked in opposite winding so the
            // even-odd rule treats it as a hole rather than an
            // extra fill.
            let inner_path = PolygonSubPath::corners(
                [
                    (inner_inset, inner_inset),
                    (inner_inset, inner),
                    (inner, inner),
                    (inner, inner_inset),
                ],
                true,
            );
            let polygon = Polygon {
                self_id: polygon_id,
                item_transform: translate((PAGE_W_PT - outer) * 0.5, (PAGE_H_PT - outer) * 0.5),
                fill_color: Some("Color/RGBMagenta".to_string()),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight_pt: Some(1.0),
                subpaths: vec![outer_path, inner_path],
                text_path: None,
            };
            vec![polygon.into()]
        }
        _ => Vec::new(),
    }
}

/// Two side-by-side filled rectangles in a Group's local frame.
/// Rect A at (0, 0); Rect B 20pt to the right of A. Both get a
/// thin black stroke so they read distinctly when overlapped by
/// rotation.
fn two_rect_pair(a_id: &str, b_id: &str) -> Vec<PageItem> {
    let gap = 20.0;
    let a = Rect {
        self_id: a_id.to_string(),
        width_pt: RECT_W_PT,
        height_pt: RECT_H_PT,
        item_transform: IDENTITY,
        fill_color: Some("Color/RGBCyan".to_string()),
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
    };
    let b = Rect {
        self_id: b_id.to_string(),
        width_pt: RECT_W_PT,
        height_pt: RECT_H_PT,
        item_transform: translate(RECT_W_PT + gap, 0.0),
        fill_color: Some("Color/RGBMagenta".to_string()),
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
    };
    vec![a.into(), b.into()]
}
