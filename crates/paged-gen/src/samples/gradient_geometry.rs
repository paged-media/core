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

//! `gradient-geometry.idml` — where a linear gradient runs, asked of
//! InDesign (ADR 120).
//!
//! The `gradients` fixture only ever drew the default 0° ramp, so
//! nothing measured which way an angle turns, where an explicit start
//! sits, or which segment a stop's `Midpoint` bends. Every gradient
//! here runs from BLACK (the first stop) to PAPER unless a case says
//! otherwise, so the dark end of a frame names its first stop.
//!
//! Page 1 — `GradientFillAngle` with no start or length (the default
//! placement), cases 1–18: angles 0, 45, 90, 135, 180 and −90 (rows) ×
//! a square 80 × 80, a 2:1 rectangle 120 × 60 and a square rotated 30°
//! counter-clockwise on the page (columns).
//!
//! Page 2 — the rest:
//! * 19 — 240 × 60 at 0° with `GradientFillStart="60 30"`,
//!   `GradientFillLength="120"` (the ramp should occupy x 60..180);
//! * 20 — 100 × 100 at 90°, start `50 80`, length 60;
//! * 21 — 160 × 80 at 45°, start `20 70`, length 100;
//! * 22–24 — three stops black · paper · black at 0 / 50 / 100 %, with
//!   every `Midpoint` at its default (22), `Midpoint="20"` on stop 1
//!   (23) and `Midpoint="80"` on stop 2 (24);
//! * 25 — a text frame's fill at 90°;
//! * 26 — a 12 pt gradient stroke, `GradientStrokeAngle="90"`, no fill;
//! * 27 — an inline anchored rectangle filled at 90°;
//! * 28 — a black rectangle with an applied gradient feather at
//!   `Angle="90"` and no start or length;
//! * 29 — the same feather with `GradientStart` at the bottom centre
//!   and `Length` equal to the height.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{AnchoredObjectSetting, EffectSetting, PageItem, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras_and_gradients, preferences_xml,
        styles_xml, ExtraGradient, GradientStop,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{compose, rotate_deg, translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "gradient-geometry";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const LABEL_FONT: &str = "Open Sans";
const LABEL_SIZE: f32 = 7.0;

/// The two-stop black → paper ramp most cases fill with.
pub const BLACK_PAPER: &str = "Gradient/BlackPaper";
/// Black · paper · black at 0 / 50 / 100 %, default midpoints.
pub const THREE_STOP: &str = "Gradient/ThreeStop";
/// The same with `Midpoint="20"` on stop 1.
pub const THREE_STOP_MID1: &str = "Gradient/ThreeStopMid1At20";
/// The same with `Midpoint="80"` on stop 2.
pub const THREE_STOP_MID2: &str = "Gradient/ThreeStopMid2At80";

/// Page 1 rows: the `GradientFillAngle` each row applies.
pub const ANGLES: [f32; 6] = [0.0, 45.0, 90.0, 135.0, 180.0, -90.0];
/// Page 1 row tops.
pub const ROW_TOP: [f32; 6] = [80.0, 205.0, 330.0, 455.0, 580.0, 705.0];
/// Page 1 shapes: `(left, width, height)`; the rotated square is
/// placed by its centre at `left + 40`.
pub const SHAPES: [(f32, f32, f32); 3] = [
    (150.0, 80.0, 80.0),
    (290.0, 120.0, 60.0),
    (450.0, 80.0, 80.0),
];
/// The page rotation of column 3, counter-clockwise, degrees.
pub const ROTATION_DEG: f32 = 30.0;

fn stop(color: &str, location_pct: f32, midpoint_pct: Option<f32>) -> GradientStop {
    GradientStop {
        stop_color: color.to_string(),
        location_pct,
        midpoint_pct,
    }
}

fn gradients() -> Vec<ExtraGradient> {
    let three = |id: &str, name: &str, m1: Option<f32>, m2: Option<f32>| ExtraGradient {
        self_id: id.to_string(),
        name: name.to_string(),
        kind: "Linear",
        stops: vec![
            stop("Color/Black", 0.0, None),
            stop("Color/Paper", 50.0, m1),
            stop("Color/Black", 100.0, m2),
        ],
    };
    vec![
        ExtraGradient {
            self_id: BLACK_PAPER.to_string(),
            name: "Black to Paper".to_string(),
            kind: "Linear",
            stops: vec![
                stop("Color/Black", 0.0, None),
                stop("Color/Paper", 100.0, None),
            ],
        },
        three(THREE_STOP, "Three stops", None, None),
        three(
            THREE_STOP_MID1,
            "Three stops, midpoint 20 on stop 1",
            Some(20.0),
            None,
        ),
        three(
            THREE_STOP_MID2,
            "Three stops, midpoint 80 on stop 2",
            None,
            Some(80.0),
        ),
    ]
}

/// A frame at `xf` (top-left at the inner origin) with no stroke.
fn frame(id: String, w: f32, h: f32, xf: Matrix, fill: Option<&str>) -> Rect {
    let mut r = Rect::filled(id, w, h, xf);
    r.fill_color = fill.map(str::to_string);
    r
}

fn label_run(text: &str) -> Run {
    Run {
        point_size: Some(LABEL_SIZE),
        applied_font: Some(LABEL_FONT),
        fill_color: Some("Color/Black".to_string()),
        ..Run::with_char_attrs(text, Vec::new())
    }
}

/// Collects a page's items and stories.
struct Page {
    items: Vec<PageItem>,
    stories: Vec<(String, Vec<u8>)>,
    seq: u32,
}

impl Page {
    fn new(seq: u32) -> Self {
        Self {
            items: Vec::new(),
            stories: Vec::new(),
            seq,
        }
    }

    fn next(&mut self, kind: &str) -> String {
        self.seq += 1;
        self_id(SAMPLE, kind, self.seq)
    }

    /// A story of `paragraphs`, returning its id.
    fn story(&mut self, paragraphs: Vec<Paragraph>) -> String {
        let id = self.next("Story");
        self.stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs,
            }),
        ));
        id
    }

    /// A one-line 7 pt label whose frame's top-left is `(x, y)`.
    fn label(&mut self, text: &str, x: f32, y: f32, w: f32) {
        let story = self.story(vec![Paragraph {
            runs: vec![label_run(text)],
            leading: Some(LABEL_SIZE * 1.2),
            ..Paragraph::plain("")
        }]);
        let id = self.next("TextFrame");
        let r = frame(id, w, 12.0, translate(x, y), None).with_parent_story(story);
        self.items.push(r.into());
    }

    fn push(&mut self, r: Rect) {
        self.items.push(r.into());
    }
}

fn page_one() -> Page {
    let mut p = Page::new(0);
    p.label(
        "gradient-geometry · page 1 · GradientFillAngle, no start, no length · black = first stop",
        36.0,
        30.0,
        520.0,
    );
    p.label("square 80 × 80", SHAPES[0].0, 60.0, 120.0);
    p.label("2:1 rectangle 120 × 60", SHAPES[1].0, 60.0, 120.0);
    p.label("square rotated 30° ccw", SHAPES[2].0, 60.0, 120.0);
    for (row, (&angle, &top)) in ANGLES.iter().zip(ROW_TOP.iter()).enumerate() {
        let first = row * 3 + 1;
        p.label(
            &format!("{}–{} · {}°", first, first + 2, angle),
            36.0,
            top + 34.0,
            100.0,
        );
        for (col, &(left, w, h)) in SHAPES.iter().enumerate() {
            let xf = if col == 2 {
                // Rotate about the square's centre, counter-clockwise on
                // the page: InDesign writes a +30° rotation as
                // `cos −sin sin cos`, which is `rotate_deg(-30)` here.
                let centre = (left + w * 0.5, top + h * 0.5);
                compose(
                    compose(translate(-w * 0.5, -h * 0.5), rotate_deg(-ROTATION_DEG)),
                    translate(centre.0, centre.1),
                )
            } else {
                translate(left, top + (80.0 - h) * 0.5)
            };
            let id = p.next("Rectangle");
            p.push(
                frame(id, w, h, xf, Some(BLACK_PAPER))
                    .with_attr("GradientFillAngle", crate::xml::format_f32(angle)),
            );
        }
    }
    p
}

fn page_two() -> Page {
    let mut p = Page::new(100);
    p.label("gradient-geometry · page 2 · start + length, midpoints, text frame, stroke, anchored, feather", 36.0, 30.0, 520.0);

    // 19–21: explicit start + length.
    p.label("19 · 0°, start 60 30, length 120", 36.0, 56.0, 240.0);
    let id = p.next("Rectangle");
    p.push(
        frame(id, 240.0, 60.0, translate(36.0, 70.0), Some(BLACK_PAPER))
            .with_attr("GradientFillAngle", "0")
            .with_attr("GradientFillStart", "60 30")
            .with_attr("GradientFillLength", "120"),
    );
    p.label("20 · 90°, start 50 80, length 60", 300.0, 56.0, 130.0);
    let id = p.next("Rectangle");
    p.push(
        frame(id, 100.0, 100.0, translate(300.0, 70.0), Some(BLACK_PAPER))
            .with_attr("GradientFillAngle", "90")
            .with_attr("GradientFillStart", "50 80")
            .with_attr("GradientFillLength", "60"),
    );
    p.label("21 · 45°, start 20 70, length 100", 430.0, 56.0, 130.0);
    let id = p.next("Rectangle");
    p.push(
        frame(id, 120.0, 80.0, translate(430.0, 70.0), Some(BLACK_PAPER))
            .with_attr("GradientFillAngle", "45")
            .with_attr("GradientFillStart", "20 70")
            .with_attr("GradientFillLength", "100"),
    );

    // 22–24: midpoints on a 360 × 40 ramp, 0°.
    let mids = [
        ("22 · black · paper · black, midpoints default", THREE_STOP),
        ("23 · Midpoint 20 on stop 1 (paper)", THREE_STOP_MID1),
        ("24 · Midpoint 80 on stop 2 (last black)", THREE_STOP_MID2),
    ];
    for (i, (text, grad)) in mids.iter().enumerate() {
        let top = 200.0 + i as f32 * 62.0;
        p.label(text, 36.0, top, 360.0);
        let id = p.next("Rectangle");
        p.push(frame(
            id,
            360.0,
            40.0,
            translate(36.0, top + 14.0),
            Some(grad),
        ));
    }

    // 25: a text frame's fill at 90°.
    p.label("25 · text frame fill, 90°", 36.0, 400.0, 200.0);
    let story = p.story(vec![Paragraph {
        runs: vec![Run {
            point_size: Some(14.0),
            ..label_run("text frame")
        }],
        leading: Some(17.0),
        ..Paragraph::plain("")
    }]);
    let id = p.next("TextFrame");
    p.push(
        frame(id, 200.0, 60.0, translate(36.0, 414.0), Some(BLACK_PAPER))
            .with_parent_story(story)
            .with_attr("GradientFillAngle", "90"),
    );

    // 26: a gradient stroke at 90°, no fill.
    p.label(
        "26 · 12 pt gradient stroke, GradientStrokeAngle 90",
        300.0,
        400.0,
        240.0,
    );
    let id = p.next("Rectangle");
    p.push(
        frame(id, 200.0, 60.0, translate(312.0, 420.0), None)
            .with_stroke(BLACK_PAPER, 12.0)
            .with_attr("GradientStrokeAngle", "90"),
    );

    // 27: an inline anchored rectangle filled at 90°.
    p.label(
        "27 · inline anchored rectangle 60 × 40, 90°",
        36.0,
        500.0,
        300.0,
    );
    let anchor_id = p.next("Rectangle");
    let mut anchored = frame(anchor_id, 60.0, 40.0, IDENTITY, Some(BLACK_PAPER))
        .with_attr("GradientFillAngle", "90");
    anchored.anchored_setting = Some(AnchoredObjectSetting::inline());
    let story = p.story(vec![Paragraph {
        runs: vec![
            Run {
                point_size: Some(12.0),
                ..label_run("before ")
            },
            Run {
                point_size: Some(12.0),
                anchored_frame: Some(anchored),
                ..label_run(" after")
            },
        ],
        leading: Some(48.0),
        ..Paragraph::plain("")
    }]);
    let id = p.next("TextFrame");
    p.push(frame(id, 300.0, 60.0, translate(36.0, 514.0), None).with_parent_story(story));

    // 28–29: gradient feather on a black rectangle.
    p.label(
        "28 · gradient feather, Angle 90, no start",
        36.0,
        600.0,
        220.0,
    );
    let id = p.next("Rectangle");
    let mut r = frame(id, 160.0, 60.0, translate(36.0, 614.0), Some("Color/Black"));
    r.frame_effects.push(EffectSetting::new(
        "GradientFeatherSetting",
        vec![("Type", "Linear".to_string()), ("Angle", "90".to_string())],
    ));
    p.push(r);
    p.label(
        "29 · gradient feather, Angle 90, start 80 60, length 60",
        300.0,
        600.0,
        260.0,
    );
    let id = p.next("Rectangle");
    let mut r = frame(
        id,
        160.0,
        60.0,
        translate(300.0, 614.0),
        Some("Color/Black"),
    );
    r.frame_effects.push(EffectSetting::new(
        "GradientFeatherSetting",
        vec![
            ("Type", "Linear".to_string()),
            ("Angle", "90".to_string()),
            ("GradientStart", "80 60".to_string()),
            ("Length", "60".to_string()),
        ],
    ));
    p.push(r);
    p
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let pages = [page_one(), page_two()];
    let mut master_spreads = Vec::new();
    let mut master_refs = Vec::new();
    let mut spreads = Vec::new();
    let mut spread_refs = Vec::new();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    for (i, page) in pages.into_iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", seq),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());
        let spread_id = self_id(SAMPLE, "Spread", seq);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: format!("gradient-geometry {}", i + 1),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: page.items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        ));
        spread_refs.push(spread_id);
        for (id, xml) in page.stories {
            story_refs.push(id.clone());
            stories.push((id, xml));
        }
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: master_refs,
            spreads: spread_refs,
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_with_extras_and_gradients(&[], &gradients()),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader: the angles, the explicit start
    /// and length, and the stored midpoints all arrive.
    #[test]
    fn the_gradient_attributes_read_back() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let p1 = &doc.spreads[0].spread;
        let angles: Vec<f32> = p1
            .rectangles
            .iter()
            .filter_map(|r| r.gradient_fill_angle)
            .collect();
        assert_eq!(angles.len(), 18);
        let p2 = &doc.spreads[1].spread;
        let r19 = p2
            .rectangles
            .iter()
            .find(|r| r.gradient_fill_length == Some(120.0))
            .expect("case 19");
        assert_eq!(r19.gradient_fill_start, Some([60.0, 30.0]));
        let g = doc
            .palette
            .gradients
            .get(THREE_STOP_MID1)
            .expect("gradient");
        assert_eq!(g.stops[1].midpoint_pct, Some(20.0));
        let g = doc
            .palette
            .gradients
            .get(THREE_STOP_MID2)
            .expect("gradient");
        assert_eq!(g.stops[2].midpoint_pct, Some(80.0));
    }
}
