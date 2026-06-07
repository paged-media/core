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

//! Phase-2 mega-file: `text-wrap.idml`.
//!
//! Each page hosts a body TextFrame full of long Lorem-ish text plus
//! one or two graphic obstacles carrying a `<TextWrapPreference>`.
//! The variants exercise the IDML wrap-mode enum and the four-edge
//! offset payload independently, so the renderer's wrap-rect
//! collector and line-breaker integration can be regression-tested
//! per mode.
//!
//! Variants:
//!   * `text-wrap · contour · rect-aligned` —
//!     `TextWrapMode="BoundingBoxTextWrap"` on an axis-aligned rect.
//!     The body text wraps around the obstacle's AABB.
//!   * `text-wrap · contour · circle` — an axis-aligned square
//!     obstacle with `TextWrapMode="ContourTextWrap"`. The renderer
//!     today reduces contour to its AABB; a future contour-aware
//!     renderer can use the same IDML.
//!   * `text-wrap · jump-object` — `JumpObjectTextWrap`. Text breaks
//!     across the obstacle and resumes on the line below it.
//!   * `text-wrap · next-column` — `NextColumnTextWrap`. Text moves
//!     to the next column past the obstacle.
//!   * `text-wrap · offsets-asymmetric` — wrap rect inflated
//!     asymmetrically: `Top=6 Left=12 Bottom=6 Right=12` so the
//!     body text steers wider on the horizontal axis.
//!   * `text-wrap · disabled` — `TextWrapMode="None"` baseline; body
//!     text flows over the obstacle's footprint as if it weren't
//!     there.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Rect, TextWrap},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text-wrap";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_W_PT: f32 = 460.0;
const BODY_H_PT: f32 = 720.0;
const OBSTACLE_W_PT: f32 = 140.0;
const OBSTACLE_H_PT: f32 = 100.0;
const LABEL_W_PT: f32 = 460.0;
const LABEL_H_PT: f32 = 24.0;

/// One page-spec — the obstacle's wrap settings + name.
struct Variant {
    name: &'static str,
    mode: &'static str,
    offsets: [f32; 4],
    side: Option<&'static str>,
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "text-wrap · contour · rect-aligned",
            mode: "BoundingBoxTextWrap",
            offsets: [6.0, 6.0, 6.0, 6.0],
            side: Some("BothSides"),
        },
        // Same square obstacle with ContourTextWrap. The renderer's
        // wrap collector reduces contour to AABB today; the IDML
        // captures the intent so a contour-aware future-renderer can
        // see the same input.
        Variant {
            name: "text-wrap · contour · circle",
            mode: "ContourTextWrap",
            offsets: [4.0, 4.0, 4.0, 4.0],
            side: Some("BothSides"),
        },
        Variant {
            name: "text-wrap · jump-object",
            mode: "JumpObjectTextWrap",
            offsets: [4.0, 0.0, 4.0, 0.0],
            side: None,
        },
        Variant {
            name: "text-wrap · next-column",
            mode: "NextColumnTextWrap",
            offsets: [4.0, 0.0, 4.0, 0.0],
            side: None,
        },
        // Asymmetric offsets — top/bottom inset 6, left/right 12 so
        // the wrap rect is visibly wider than the obstacle. Catches
        // renderers that confuse the [top, left, bottom, right]
        // attribute order.
        Variant {
            name: "text-wrap · offsets-asymmetric",
            mode: "BoundingBoxTextWrap",
            offsets: [6.0, 12.0, 6.0, 12.0],
            side: Some("BothSides"),
        },
        // Sanity baseline: TextWrapMode="None" → text flows over
        // the obstacle as if no wrap were declared.
        Variant {
            name: "text-wrap · disabled",
            mode: "None",
            offsets: [0.0, 0.0, 0.0, 0.0],
            side: Some("BothSides"),
        },
    ]
}

/// Lorem-ish body text long enough to fill ~720 pt of body frame at
/// 12 pt size with default leading. Two paragraphs so paragraph
/// breaks are visible in the diff harness.
fn body_paragraphs() -> Vec<Paragraph> {
    let p1 = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod \
              tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim \
              veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea \
              commodo consequat. Duis aute irure dolor in reprehenderit in voluptate \
              velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint \
              occaecat cupidatat non proident, sunt in culpa qui officia deserunt \
              mollit anim id est laborum.";
    let p2 = "Curabitur pretium tincidunt lacus. Nulla gravida orci a odio. Nullam \
              varius, turpis et commodo pharetra, est eros bibendum elit, nec luctus \
              magna felis sollicitudin mauris. Integer in mauris eu nibh euismod \
              gravida. Duis ac tellus et risus vulputate vehicula. Donec lobortis \
              risus a elit. Etiam tempor. Ut ullamcorper, ligula eu tempor congue, \
              eros est euismod turpis, id tincidunt sapien risus a quam. Maecenas \
              fermentum consequat mi.";
    vec![Paragraph::plain(p1), Paragraph::plain(p2)]
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
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let body_story_id = self_id(SAMPLE, "BodyStory", seq);
        let label_frame_id = self_id(SAMPLE, "LabelFrame", seq);
        let body_frame_id = self_id(SAMPLE, "BodyFrame", seq);
        let obstacle_id = self_id(SAMPLE, "Obstacle", seq);

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

        // Label story.
        stories.push((
            label_story_id.clone(),
            write_story(&Story {
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(label_story_id.clone());

        // Body story (the wrapping target).
        stories.push((
            body_story_id.clone(),
            write_story(&Story {
                self_id: body_story_id.clone(),
                paragraphs: body_paragraphs(),
            }),
        ));
        story_refs.push(body_story_id.clone());

        let label = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(label_story_id),
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

        // Body frame fills most of the page. Centred horizontally.
        let body_x = (PAGE_W_PT - BODY_W_PT) * 0.5;
        let body_y = 80.0; // below the label
        let body = Rect {
            self_id: body_frame_id,
            width_pt: BODY_W_PT,
            height_pt: BODY_H_PT,
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
        };

        // Obstacle — yellow rectangle floating over the body. Sits
        // around 1/3 down so wrap effects show in both halves of the
        // body text.
        let ox = body_x + (BODY_W_PT - OBSTACLE_W_PT) * 0.5;
        let oy = body_y + 200.0;
        let obstacle = Rect {
            self_id: obstacle_id,
            width_pt: OBSTACLE_W_PT,
            height_pt: OBSTACLE_H_PT,
            item_transform: translate(ox, oy),
            fill_color: Some("Color/Paper".to_string()),
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
            parent_story: None,
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: Some(TextWrap {
                mode: variant.mode,
                offsets: variant.offsets,
                side: variant.side,
            }),
            anchored_setting: None,
            frame_effects: Vec::new(),
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
                // Z-order: body first so the obstacle paints on top.
                page_items: vec![label.into(), body.into(), obstacle.into()],
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

    // Suppress unused-import warning if the variant matrix happens
    // to use only `translate`.
    let _: Matrix = IDENTITY;

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
