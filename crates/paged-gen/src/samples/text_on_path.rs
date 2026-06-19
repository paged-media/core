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

//! Mega-file: `text-on-path.idml` — text flowing along a shape path.
//!
//! Six single-page spreads, each a host `<Polygon>` carrying a
//! `<TextPath>` child that references a story:
//!
//!   1. `arch` — an open cubic-Bézier arch. Default Baseline alignment,
//!      Rainbow effect; the text centres along the arc and each glyph
//!      rotates to the local tangent.
//!   2. `circle` — a closed circle (four Bézier quarter-arcs). Uses
//!      `CenterPathType` to seat each glyph's em-box centre on the
//!      ring rather than its baseline.
//!   3. `overset` — a short straight path with far more text than fits.
//!      The trailing glyphs drop and the renderer fires an
//!      `OversetTextDropped` diagnostic (matching the body-text
//!      overset contract).
//!   4. `segment · baseline`, 5. `segment · ascender`, 6. `segment ·
//!      descender` — three identical straight horizontal segments
//!      carrying the same text under the remaining `PathTypeAlignment`
//!      seats. On a flat path the seat shift is purely vertical, so the
//!      conformance corpus can diff Ascender / Descender against
//!      Baseline directly.
//!
//! The host polygons carry no fill (`Swatch/None`) but a thin black
//! stroke so the curve is visible in the exported PDF; the glyphs ride
//! the same geometry the stroke draws.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, PathPoint, Polygon, PolygonSubPath, TextPathChild},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text-on-path";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const BODY_FONT: &str = "Open Sans";
const TEXT_PT: f32 = 18.0;

/// Kappa — the Bézier handle length (as a fraction of the radius) that
/// approximates a quarter circle. Standard circle-from-cubics constant.
const KAPPA: f32 = 0.552_284_8;

/// One styled run pinned to the body font so shaping is deterministic.
fn run(text: &str) -> Run {
    Run {
        extra_char_attrs: Vec::new(),
        text: text.to_string(),
        point_size: Some(TEXT_PT),
        fill_color: Some("Color/Black".to_string()),
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: Some(BODY_FONT),
        anchored_frame: None,
    }
}

/// One single-run paragraph carrying `text`.
fn para(text: &str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        justification: None,
        applied_numbering_list: None,
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
        bullet_character: None,
        table: None,
        minimum_letter_spacing: None,
        desired_letter_spacing: None,
        maximum_letter_spacing: None,
        runs: vec![run(text)],
    }
}

/// A page in the fixture: a host polygon (its geometry is the path) and
/// the text that rides it, plus the alignment / effect knobs.
struct Variant {
    name: &'static str,
    /// Polygon sub-path (open arch, closed ring, or short segment).
    subpath: PolygonSubPath,
    /// Story text — one paragraph.
    text: &'static str,
    path_type_alignment: Option<&'static str>,
    path_effect: Option<&'static str>,
    start_bracket: Option<f32>,
    end_bracket: Option<f32>,
}

/// The open arch sub-path: a symmetric cubic from (0,120)→(400,120)
/// bulging up to ~(200,0). Inner coords; the polygon's ItemTransform
/// drops it onto the page. Two Bézier points with mirrored handles.
fn arch_subpath() -> PolygonSubPath {
    PolygonSubPath {
        points: vec![
            // Left end, handle pulling up-and-right toward the apex.
            PathPoint::curve((0.0, 120.0), (0.0, 120.0), (120.0, 0.0)),
            // Right end, handle pulling up-and-left toward the apex.
            PathPoint::curve((400.0, 120.0), (280.0, 0.0), (400.0, 120.0)),
        ],
        closed: false,
    }
}

/// A closed circle of radius `r` centred at the local origin, built
/// from four cubic quarter-arcs. Walked clockwise from the right
/// (3 o'clock) so the text reads left-to-right across the top.
fn circle_subpath(r: f32) -> PolygonSubPath {
    let k = r * KAPPA;
    PolygonSubPath {
        points: vec![
            // 3 o'clock.
            PathPoint::curve((r, 0.0), (r, -k), (r, k)),
            // 6 o'clock (bottom; +y is down in IDML).
            PathPoint::curve((0.0, r), (k, r), (-k, r)),
            // 9 o'clock.
            PathPoint::curve((-r, 0.0), (-r, k), (-r, -k)),
            // 12 o'clock (top).
            PathPoint::curve((0.0, -r), (-k, -r), (k, -r)),
        ],
        closed: true,
    }
}

/// A short straight horizontal segment of `len` pt from the origin —
/// far shorter than the text that rides it, to force overset.
fn short_segment(len: f32) -> PolygonSubPath {
    PolygonSubPath {
        points: vec![PathPoint::corner((0.0, 0.0)), PathPoint::corner((len, 0.0))],
        closed: false,
    }
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "text-on-path · arch · baseline · rainbow",
            subpath: arch_subpath(),
            text: "Type along the arch",
            path_type_alignment: Some("BaselinePathType"),
            path_effect: Some("RainbowPathEffect"),
            start_bracket: None,
            end_bracket: None,
        },
        Variant {
            name: "text-on-path · circle · center",
            subpath: circle_subpath(140.0),
            text: "Around and around the ring we go",
            path_type_alignment: Some("CenterPathType"),
            path_effect: Some("RainbowPathEffect"),
            start_bracket: None,
            end_bracket: None,
        },
        Variant {
            // ~80 pt of path, ~30 glyphs of 18 pt text ⇒ overset tail.
            name: "text-on-path · overset · short-segment",
            subpath: short_segment(80.0),
            text: "This sentence is far too long to fit on a tiny path",
            path_type_alignment: Some("BaselinePathType"),
            path_effect: Some("RainbowPathEffect"),
            start_bracket: Some(0.0),
            end_bracket: Some(80.0),
        },
        // The remaining two PathTypeAlignment seats the renderer honours
        // (`AscenderPathType` lifts each glyph so its top rides the path;
        // `DescenderPathType` drops it so its bottom does). Both ride a
        // STRAIGHT horizontal segment carrying the SAME text as the
        // baseline-segment page below, so the seat shift is purely
        // vertical and the conformance corpus / emission test can diff
        // the seat alone — the alignment seats the text-on-path fixture
        // was missing (W1.6 alignment coverage).
        Variant {
            name: "text-on-path · segment · baseline",
            subpath: short_segment(360.0),
            text: SEAT_TEXT,
            path_type_alignment: Some("BaselinePathType"),
            path_effect: Some("RainbowPathEffect"),
            start_bracket: None,
            end_bracket: None,
        },
        Variant {
            name: "text-on-path · segment · ascender",
            subpath: short_segment(360.0),
            text: SEAT_TEXT,
            path_type_alignment: Some("AscenderPathType"),
            path_effect: Some("RainbowPathEffect"),
            start_bracket: None,
            end_bracket: None,
        },
        Variant {
            name: "text-on-path · segment · descender",
            subpath: short_segment(360.0),
            text: SEAT_TEXT,
            path_type_alignment: Some("DescenderPathType"),
            path_effect: Some("RainbowPathEffect"),
            start_bracket: None,
            end_bracket: None,
        },
    ]
}

/// Shared text for the three straight-segment seat pages, short enough
/// to fit the 360 pt path so no glyph oversets.
const SEAT_TEXT: &str = "Seat the glyphs";

/// Build the full `Sample` ready for `write_idml`.
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
        let poly_id = self_id(SAMPLE, "Polygon", seq);
        let tp_id = self_id(SAMPLE, "TextPath", seq);

        // Master — empty.
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

        // Story whose text rides the path. Text-on-path is a single
        // baseline along the curve, so one paragraph is enough.
        stories.push((
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![para(variant.text)],
            }),
        ));
        story_refs.push(story_id.clone());

        // Host polygon: its `<PathGeometry>` is the path; the
        // `<TextPath>` child links the story. Centre the local geometry
        // on the page. The circle sits at its centre; the arch / segment
        // are local-origin shapes so we offset to roughly centre them.
        let poly = Polygon {
            self_id: poly_id,
            item_transform: translate(PAGE_W_PT * 0.5, PAGE_H_PT * 0.5),
            fill_color: None,
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(1.0),
            extra_attrs: Vec::new(),
            subpaths: vec![variant.subpath],
            text_path: Some(TextPathChild {
                self_id: tp_id,
                parent_story: story_id.clone(),
                path_type_alignment: variant.path_type_alignment,
                path_effect: variant.path_effect,
                start_bracket: variant.start_bracket,
                end_bracket: variant.end_bracket,
            }),
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
                page_items: vec![PageItem::from(poly)],
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
