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

//! `text-in-shape.idml` — text LAYOUT inside non-rectangular frames
//! (W1.10, wrap-INSIDE).
//!
//! Three pages, each a single TextFrame whose `<PathGeometry>` is a
//! non-rectangular outline filled with center-justified body text:
//!
//!   * `text-in-shape · oval` — an ellipse (four cardinal anchors with
//!     Bezier handles). Lines near the top/bottom are short; the line
//!     across the equator runs nearly the full width.
//!   * `text-in-shape · triangle` — an apex-up triangle. Line width
//!     grows monotonically from the narrow apex to the wide base.
//!   * `text-in-shape · donut` — a compound path: an outer ring with an
//!     inner hole (the hole contour wound opposite the outer). Lines
//!     crossing the hole's y-band split into two segments; the widest
//!     is used (v1 single-segment policy).
//!
//! The fixture exercises insets, `FirstBaselineOffset`, and a
//! `VerticalJustification` other than Top so the shape/VJ interplay is
//! visible. Fixtures are gitignored — regenerate with
//! `cargo run -p paged-gen -- emit text-in-shape <out_dir>` before
//! consuming the pipeline tests.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PathPoint, PolygonSubPath, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text-in-shape";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

/// Shape extent (inner-coord bounding box, top-left at origin).
const SHAPE_W_PT: f32 = 320.0;
const SHAPE_H_PT: f32 = 320.0;
/// Kappa constant: control-handle length / radius for a quarter-circle
/// cubic Bezier approximation. The renderer flattens these handles.
const KAPPA: f32 = 0.552_284_8;

/// Body text — short words so many lines fit and the shape's per-line
/// segment shows in each line's centre / split. Repeated enough to fill
/// the shape's full height (excess overflows as overset, clipped).
fn body_text() -> String {
    let unit = "the quick brown fox jumps over a lazy dog and then the \
                slow red cat naps on a warm sunny windowsill all day long ";
    unit.repeat(12)
}

/// One page-spec.
struct Variant {
    name: &'static str,
    subpaths: Vec<PolygonSubPath>,
}

/// Ellipse inscribed in `SHAPE_W × SHAPE_H` as four cardinal anchors
/// with Bezier handles (the shape InDesign writes for an oval).
fn oval_subpaths() -> Vec<PolygonSubPath> {
    let (w, h) = (SHAPE_W_PT, SHAPE_H_PT);
    let (cx, cy) = (w * 0.5, h * 0.5);
    let (rx, ry) = (w * 0.5, h * 0.5);
    let (kx, ky) = (KAPPA * rx, KAPPA * ry);
    // Anchors: top, right, bottom, left (clockwise). Each handle pulls
    // the cubic toward the neighbouring cardinal point.
    let top = (cx, cy - ry);
    let right = (cx + rx, cy);
    let bottom = (cx, cy + ry);
    let left = (cx - rx, cy);
    let points = vec![
        PathPoint::curve(top, (cx - kx, cy - ry), (cx + kx, cy - ry)),
        PathPoint::curve(right, (cx + rx, cy - ky), (cx + rx, cy + ky)),
        PathPoint::curve(bottom, (cx + kx, cy + ry), (cx - kx, cy + ry)),
        PathPoint::curve(left, (cx - rx, cy + ky), (cx - rx, cy - ky)),
    ];
    vec![PolygonSubPath {
        points,
        closed: true,
    }]
}

/// Apex-up triangle: apex at top-center, base along the bottom edge.
fn triangle_subpaths() -> Vec<PolygonSubPath> {
    let (w, h) = (SHAPE_W_PT, SHAPE_H_PT);
    vec![PolygonSubPath::corners(
        [(w * 0.5, 0.0), (w, h), (0.0, h)],
        true,
    )]
}

/// Donut: an outer square ring with a centered square hole. The hole
/// contour is wound opposite the outer so a NonZero clip fill carves
/// it; the even-odd layout test is winding-agnostic.
fn donut_subpaths() -> Vec<PolygonSubPath> {
    let (w, h) = (SHAPE_W_PT, SHAPE_H_PT);
    // Outer ring, clockwise.
    let outer = PolygonSubPath::corners([(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)], true);
    // Inner hole, counter-clockwise (reverse winding).
    let (hl, ht, hr, hb) = (w * 0.32, h * 0.32, w * 0.68, h * 0.68);
    let hole = PolygonSubPath::corners([(hl, ht), (hl, hb), (hr, hb), (hr, ht)], true);
    vec![outer, hole]
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "text-in-shape · oval",
            subpaths: oval_subpaths(),
        },
        Variant {
            name: "text-in-shape · triangle",
            subpaths: triangle_subpaths(),
        },
        Variant {
            name: "text-in-shape · donut",
            subpaths: donut_subpaths(),
        },
    ]
}

/// Center-aligned body paragraph at 11pt. Each line centres on the
/// midpoint of the shape's available segment at that band, so a line
/// crossing the donut's hole splits and re-centres in the left/right
/// gaps, and every glyph stays inside the outline.
fn body_paragraph() -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        justification: Some("CenterAlign"),
        runs: vec![Run {
            text: body_text(),
            point_size: Some(11.0),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: None,
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
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
        let body_story_id = self_id(SAMPLE, "BodyStory", seq);
        let body_frame_id = self_id(SAMPLE, "BodyFrame", seq);

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

        // The shaped text frame's story.
        stories.push((
            body_story_id.clone(),
            write_story(&Story {
                self_id: body_story_id.clone(),
                paragraphs: vec![body_paragraph()],
            }),
        ));
        story_refs.push(body_story_id.clone());

        // Centre the shape on the page.
        let fx = (PAGE_W_PT - SHAPE_W_PT) * 0.5;
        let fy = (PAGE_H_PT - SHAPE_H_PT) * 0.5;
        let frame = Rect {
            self_id: body_frame_id,
            width_pt: SHAPE_W_PT,
            height_pt: SHAPE_H_PT,
            item_transform: translate(fx, fy),
            fill_color: Some("Color/Paper".to_string()),
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.75),
            parent_story: Some(body_story_id),
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
            text_frame_pref: Some(TextFramePref {
                inset_spacing: Some([4.0, 4.0, 4.0, 4.0]),
                vertical_justification: Some("TopAlign"),
                first_baseline_offset: Some("AscentOffset"),
                ..Default::default()
            }),
            frame_effects: Vec::new(),
            custom_subpaths: Some(variant.subpaths),
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
                page_items: vec![frame.into()],
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
