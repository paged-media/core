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

//! `shaped-bands.idml` — how InDesign fits lines into SHAPED
//! (non-rectangular) text frames, case by case, so every line's band
//! can be read off InDesign's own layout.
//!
//! Eighteen frames on three pages, all Inter 10/12 with hyphenation off
//! (an exact 12 pt line grid): a circle, an apex-up triangle, a square
//! donut whose hole splits the middle lines in two, a chamfered
//! rectangle, a rectangle with a notch cut into its right side (every
//! edge on a whole point) and a narrow ellipse whose bands are too thin
//! for a word near its poles. Each shape is set left-aligned (the line's
//! start IS the band's left edge) and right-aligned (its end is the
//! band's right edge), with `AscentOffset` and `LeadingOffset` first
//! baselines, with insets, and with painted strokes (which InDesign
//! folds into the inset). The donut also carries centred and justified
//! copy, which shows how a line split by the hole is aligned; the
//! chamfer, the notch and the ellipse carry several short paragraphs,
//! which shows where a LATER paragraph's first line is measured from.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, PathPoint, PolygonSubPath, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "shaped-bands";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
pub const LEADING: f32 = 12.0;
/// Cases per page: two columns of three rows.
pub const PER_PAGE: usize = 6;
/// Control-handle length / radius of a quarter-circle cubic.
const KAPPA: f32 = 0.552_284_8;

/// The outline a case's frame carries, in frame-local points.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    /// An ellipse `w × h` as InDesign writes one: four cardinal anchors
    /// with κ handles.
    Oval { w: f32, h: f32 },
    /// An apex-up triangle `w × h`.
    Triangle { w: f32, h: f32 },
    /// A `w × h` square ring around the hole `[left, top, right, bottom]`.
    Donut { w: f32, h: f32, hole: [f32; 4] },
    /// A `w × h` rectangle with its top-left corner cut `cut` deep at
    /// 45° and its bottom-right corner cut `cut` deep too.
    Chamfer { w: f32, h: f32, cut: f32 },
    /// A `w × h` rectangle with a notch cut into its right side,
    /// `depth` deep, between `top` and `bottom`.
    Notch {
        w: f32,
        h: f32,
        depth: f32,
        top: f32,
        bottom: f32,
    },
}

impl Shape {
    pub fn size(&self) -> (f32, f32) {
        match *self {
            Shape::Oval { w, h }
            | Shape::Triangle { w, h }
            | Shape::Donut { w, h, .. }
            | Shape::Chamfer { w, h, .. }
            | Shape::Notch { w, h, .. } => (w, h),
        }
    }

    fn subpaths(&self) -> Vec<PolygonSubPath> {
        match *self {
            Shape::Oval { w, h } => {
                let (cx, cy, rx, ry) = (w * 0.5, h * 0.5, w * 0.5, h * 0.5);
                let (kx, ky) = (KAPPA * rx, KAPPA * ry);
                vec![PolygonSubPath {
                    points: vec![
                        PathPoint::curve((cx, 0.0), (cx - kx, 0.0), (cx + kx, 0.0)),
                        PathPoint::curve((w, cy), (w, cy - ky), (w, cy + ky)),
                        PathPoint::curve((cx, h), (cx + kx, h), (cx - kx, h)),
                        PathPoint::curve((0.0, cy), (0.0, cy + ky), (0.0, cy - ky)),
                    ],
                    closed: true,
                }]
            }
            Shape::Triangle { w, h } => {
                vec![PolygonSubPath::corners(
                    [(w * 0.5, 0.0), (w, h), (0.0, h)],
                    true,
                )]
            }
            Shape::Donut { w, h, hole } => {
                let [l, t, r, b] = hole;
                vec![
                    PolygonSubPath::corners([(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)], true),
                    // The hole wound the other way, as InDesign writes a
                    // compound path's inner contour.
                    PolygonSubPath::corners([(l, t), (l, b), (r, b), (r, t)], true),
                ]
            }
            Shape::Chamfer { w, h, cut } => vec![PolygonSubPath::corners(
                [
                    (cut, 0.0),
                    (w, 0.0),
                    (w, h - cut),
                    (w - cut, h),
                    (0.0, h),
                    (0.0, cut),
                ],
                true,
            )],
            Shape::Notch {
                w,
                h,
                depth,
                top,
                bottom,
            } => vec![PolygonSubPath::corners(
                [
                    (0.0, 0.0),
                    (w, 0.0),
                    (w, top),
                    (w - depth, top),
                    (w - depth, bottom),
                    (w, bottom),
                    (w, h),
                    (0.0, h),
                ],
                true,
            )],
        }
    }
}

/// What a case's story holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Body {
    /// One long paragraph in this `Justification`.
    Long(&'static str),
    /// Short numbered paragraphs, alternately left- and right-aligned.
    Paragraphs,
}

pub struct Case {
    pub name: &'static str,
    pub shape: Shape,
    pub copy: Body,
    /// `FirstBaselineOffset`.
    pub first_baseline: &'static str,
    /// `InsetSpacing`, one value on all four sides.
    pub inset: f32,
    /// A centred black stroke of this weight; `0` paints none.
    pub stroke: f32,
}

const CIRCLE: Shape = Shape::Oval { w: 200.0, h: 200.0 };
const TRIANGLE: Shape = Shape::Triangle { w: 220.0, h: 200.0 };
const DONUT: Shape = Shape::Donut {
    w: 220.0,
    h: 200.0,
    hole: [70.0, 60.0, 150.0, 140.0],
};
const CHAMFER: Shape = Shape::Chamfer {
    w: 220.0,
    h: 200.0,
    cut: 60.0,
};
const NOTCH: Shape = Shape::Notch {
    w: 220.0,
    h: 200.0,
    depth: 90.0,
    top: 54.0,
    bottom: 126.0,
};
const NARROW: Shape = Shape::Oval { w: 90.0, h: 200.0 };

const ASCENT: &str = "AscentOffset";
const LEADING_OFFSET: &str = "LeadingOffset";
const LEFT: Body = Body::Long("LeftAlign");
const RIGHT: Body = Body::Long("RightAlign");

const fn case(name: &'static str, shape: Shape, copy: Body) -> Case {
    Case {
        name,
        shape,
        copy,
        first_baseline: ASCENT,
        inset: 0.0,
        stroke: 0.0,
    }
}

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    vec![
        case("circle, left", CIRCLE, LEFT),
        case("circle, right", CIRCLE, RIGHT),
        Case {
            first_baseline: LEADING_OFFSET,
            inset: 6.0,
            ..case("circle, left, inset 6, leading offset", CIRCLE, LEFT)
        },
        Case {
            first_baseline: LEADING_OFFSET,
            stroke: 4.0,
            ..case("circle, right, 4 pt stroke, leading offset", CIRCLE, RIGHT)
        },
        case("triangle, left", TRIANGLE, LEFT),
        Case {
            inset: 3.0,
            stroke: 2.0,
            ..case("triangle, right, inset 3 + 2 pt stroke", TRIANGLE, RIGHT)
        },
        case("donut, left", DONUT, LEFT),
        case("donut, right", DONUT, RIGHT),
        case("donut, centre", DONUT, Body::Long("CenterAlign")),
        case("donut, justified", DONUT, Body::Long("LeftJustified")),
        Case {
            inset: 4.0,
            stroke: 0.75,
            ..case(
                "donut, centre, inset 4 + 0.75 pt stroke",
                DONUT,
                Body::Long("CenterAlign"),
            )
        },
        Case {
            stroke: 6.0,
            ..case("donut, left, 6 pt stroke", DONUT, LEFT)
        },
        case("chamfer, paragraphs", CHAMFER, Body::Paragraphs),
        Case {
            first_baseline: LEADING_OFFSET,
            stroke: 6.0,
            ..case(
                "chamfer, paragraphs, 6 pt stroke, leading offset",
                CHAMFER,
                Body::Paragraphs,
            )
        },
        case("notch, left", NOTCH, LEFT),
        Case {
            stroke: 1.0,
            ..case("notch, right, 1 pt stroke", NOTCH, RIGHT)
        },
        case("narrow oval, left", NARROW, LEFT),
        case("narrow oval, paragraphs", NARROW, Body::Paragraphs),
    ]
}

const WORDS: [&str; 26] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima", "mike", "november", "oscar", "papa", "quebec", "romeo", "sierra", "tango",
    "uniform", "victor", "whiskey", "xray", "yankee", "zulu",
];

/// The long paragraph: the alphabet four times over.
pub fn long_text() -> String {
    let mut words = Vec::new();
    for _ in 0..4 {
        words.extend(WORDS);
    }
    words.join(" ")
}

/// How many short paragraphs a `Body::Paragraphs` story holds.
pub const PARAGRAPHS: usize = 12;

/// Short paragraph `k` (0-based): its number and three to six words.
pub fn paragraph_text(k: usize) -> String {
    let n = 3 + (k * 5) % 4;
    let mut words = vec![format!("{}.", k + 1)];
    words.extend((0..n).map(|j| WORDS[(k * 7 + j) % WORDS.len()].to_string()));
    words.join(" ")
}

/// The `Justification` of paragraph `k` of case `c`.
pub fn justification(c: &Case, k: usize) -> &'static str {
    match c.copy {
        Body::Long(j) => j,
        Body::Paragraphs if k % 2 == 1 => "RightAlign",
        Body::Paragraphs => "LeftAlign",
    }
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// The 0-based page case `i` sits on.
pub fn page_of(i: usize) -> usize {
    i / PER_PAGE
}

/// Top-left of case `i`'s frame, page-local pt.
pub fn frame_origin(i: usize) -> (f32, f32) {
    let k = i % PER_PAGE;
    (60.0 + (k % 2) as f32 * 260.0, 60.0 + (k / 2) as f32 * 260.0)
}

fn paragraph(text: String, justification: &'static str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(LEADING),
        justification: Some(justification),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text,
            point_size: Some(POINT_SIZE),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

fn paragraphs(c: &Case) -> Vec<Paragraph> {
    match c.copy {
        Body::Long(j) => vec![paragraph(long_text(), j)],
        Body::Paragraphs => (0..PARAGRAPHS)
            .map(|k| paragraph(paragraph_text(k), justification(c, k)))
            .collect(),
    }
}

fn label(i: usize, story: String, at: (f32, f32)) -> PageItem {
    Rect {
        self_id: self_id(SAMPLE, "LabelFrame", i as u32),
        width_pt: 240.0,
        height_pt: 14.0,
        item_transform: translate(at.0, at.1 - 18.0),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story),
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

fn body_frame(i: usize, c: &Case, story: String, at: (f32, f32)) -> PageItem {
    let (w, h) = c.shape.size();
    Rect {
        self_id: self_id(SAMPLE, "Frame", i as u32),
        width_pt: w,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        stroke_color: (c.stroke > 0.0).then(|| "Color/Black".to_string()),
        stroke_weight_pt: (c.stroke > 0.0).then_some(c.stroke),
        parent_story: Some(story),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: Some(TextFramePref {
            inset_spacing: Some([c.inset; 4]),
            first_baseline_offset: Some(c.first_baseline),
            ..Default::default()
        }),
        custom_subpaths: Some(c.shape.subpaths()),
    }
    .into()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let cases = cases();
    let pages = cases.len().div_ceil(PER_PAGE);
    let mut items: Vec<Vec<PageItem>> = (0..pages).map(|_| Vec::new()).collect();

    for (i, c) in cases.iter().enumerate() {
        let at = frame_origin(i);
        let label_story = self_id(SAMPLE, "LabelStory", i as u32);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph::plain(c.name)],
            }),
        ));
        story_refs.push(label_story.clone());
        items[page_of(i)].push(label(i, label_story, at));

        let body = body_story_id(i as u32);
        stories.push((
            body.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body.clone(),
                paragraphs: paragraphs(c),
            }),
        ));
        story_refs.push(body.clone());
        items[page_of(i)].push(body_frame(i, c, body, at));
    }

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let mut spreads = Vec::new();
    let mut spread_refs = Vec::new();
    for (p, page_items) in items.into_iter().enumerate() {
        let seq = p as u32;
        let spread_id = self_id(SAMPLE, "Spread", seq);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: (seq + 1).to_string(),
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

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spread_refs,
            stories: story_refs,
        }),
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
