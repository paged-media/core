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

//! `text-wrap-sides.idml` — which side of an obstacle text wraps to,
//! inverse and ignored wraps, contour wraps and master-page wraps,
//! asked of InDesign (R-30, R-31).
//!
//! A facing Letter document, one case per page: a 468 × 612 body frame
//! of 12 pt text (hyphenation off) with one obstacle in it. Page 1 is a
//! recto alone in its spread; then versos (even) and rectos (odd).
//!
//! 1. `BothSides`, a 120 × 100 rectangle centred in the column.
//! 2. `LeftSide`. 3. `RightSide`.
//! 4. `LargestArea`, the rectangle pushed right (verso).
//! 5. `LargestArea`, the rectangle pushed left (recto).
//! 6. `SideTowardsSpine` on a verso. 7. … on a recto.
//! 8. `SideAwayFromSpine` on a verso. 9. … on a recto.
//! 10. The body frame's `IgnoreWrap="true"`.
//! 11. A contour wrap round a 160 × 120 oval (`GraphicFrame`).
//! 12. A contour wrap round a 160 × 140 apex-up triangle.
//! 13. The rectangle on the page's MASTER, wrapping its text.
//! 14. The same with `ApplyToMasterPageOnly="true"`.
//! 15. `Inverse="true"` — last, so the gate can stop before it: InDesign
//!     sets the text inside the rectangle, which the engine does not
//!     model yet (ADR 151).
//!
//! Every wrap is offset 6 pt on all sides.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_facing_master, FacingMaster},
    page_item::{PageItem, PathPoint, PolygonSubPath, Rect, TextFramePref, TextWrap},
    resources::{container_xml, fonts_xml, graphic_xml, styles_xml},
    spread::{write_facing_spread, write_spread, FacingPage, FacingSpread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "text-wrap-sides";
const PAGE_W_PT: f32 = 612.0; // Letter
const PAGE_H_PT: f32 = 792.0;
const BODY_X: f32 = 72.0;
const BODY_Y: f32 = 108.0;
const BODY_W: f32 = 468.0;
const BODY_H: f32 = 612.0;
const OBST_W: f32 = 120.0;
const OBST_H: f32 = 100.0;
const OBST_Y: f32 = 300.0;
const OFFSET: f32 = 6.0;
const KAPPA: f32 = 0.552_284_8;

/// The obstacle's outline.
#[derive(Clone, Copy)]
enum Shape {
    /// A rectangle whose left edge sits `x` in from the body's left.
    Rect(f32),
    Oval,
    Triangle,
}

struct Case {
    label: &'static str,
    shape: Shape,
    mode: &'static str,
    side: Option<&'static str>,
    inverse: bool,
    contour: Option<&'static str>,
    ignore_wrap: bool,
    /// `Some(master_only)`: the obstacle lives on the page's master.
    on_master: Option<bool>,
}

const CENTRED: f32 = (BODY_W - OBST_W) * 0.5;

fn case(label: &'static str, side: &'static str) -> Case {
    Case {
        label,
        shape: Shape::Rect(CENTRED),
        mode: "BoundingBoxTextWrap",
        side: Some(side),
        inverse: false,
        contour: None,
        ignore_wrap: false,
        on_master: None,
    }
}

fn cases() -> Vec<Case> {
    vec![
        case("1 · BothSides", "BothSides"),
        case("2 · LeftSide", "LeftSide"),
        case("3 · RightSide", "RightSide"),
        Case {
            shape: Shape::Rect(BODY_W - OBST_W - 60.0),
            ..case("4 · LargestArea, obstacle right", "LargestArea")
        },
        Case {
            shape: Shape::Rect(60.0),
            ..case("5 · LargestArea, obstacle left", "LargestArea")
        },
        case("6 · SideTowardsSpine (verso)", "SideTowardsSpine"),
        case("7 · SideTowardsSpine (recto)", "SideTowardsSpine"),
        case("8 · SideAwayFromSpine (verso)", "SideAwayFromSpine"),
        case("9 · SideAwayFromSpine (recto)", "SideAwayFromSpine"),
        Case {
            ignore_wrap: true,
            ..case("10 · body frame IgnoreWrap", "BothSides")
        },
        Case {
            shape: Shape::Oval,
            mode: "Contour",
            contour: Some("GraphicFrame"),
            ..case("11 · contour, oval", "BothSides")
        },
        Case {
            shape: Shape::Triangle,
            mode: "Contour",
            contour: Some("GraphicFrame"),
            ..case("12 · contour, triangle", "BothSides")
        },
        Case {
            on_master: Some(false),
            ..case("13 · on the master", "BothSides")
        },
        Case {
            on_master: Some(true),
            ..case("14 · on the master, ApplyToMasterPageOnly", "BothSides")
        },
        Case {
            inverse: true,
            ..case("15 · Inverse", "BothSides")
        },
    ]
}

/// Long English text, hyphenation off: the wrap is what is measured.
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
    let no_hyphens = || vec![("Hyphenation", "false")];
    (0..4)
        .flat_map(|_| {
            [
                Paragraph::plain(p1).with_para_attrs(no_hyphens()),
                Paragraph::plain(p2).with_para_attrs(no_hyphens()),
            ]
        })
        .collect()
}

/// A bare `w × h` rectangle at the origin.
fn blank(id: String, w: f32, h: f32) -> Rect {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(0.0, 0.0),
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

fn text_frame(id: String, story: String, size: (f32, f32), at: (f32, f32)) -> Rect {
    let mut r = blank(id, size.0, size.1);
    r.parent_story = Some(story);
    r.item_transform = translate(at.0, at.1);
    r
}

/// The obstacle, at page-relative `dx` (the verso sits at −PAGE_W).
fn obstacle(id: String, c: &Case, dx: f32) -> Rect {
    let (w, h, x, subpaths) = match c.shape {
        Shape::Rect(x) => (OBST_W, OBST_H, x, None),
        Shape::Oval => {
            let (w, h) = (160.0_f32, 120.0_f32);
            let (cx, cy, rx, ry) = (w * 0.5, h * 0.5, w * 0.5, h * 0.5);
            let (kx, ky) = (KAPPA * rx, KAPPA * ry);
            let points = vec![
                PathPoint::curve((cx, 0.0), (cx - kx, 0.0), (cx + kx, 0.0)),
                PathPoint::curve((w, cy), (w, cy - ky), (w, cy + ky)),
                PathPoint::curve((cx, h), (cx + kx, h), (cx - kx, h)),
                PathPoint::curve((0.0, cy), (0.0, cy + ky), (0.0, cy - ky)),
            ];
            (
                w,
                h,
                (BODY_W - w) * 0.5,
                Some(vec![PolygonSubPath {
                    points,
                    closed: true,
                }]),
            )
        }
        Shape::Triangle => {
            let (w, h) = (160.0_f32, 140.0_f32);
            (
                w,
                h,
                (BODY_W - w) * 0.5,
                Some(vec![PolygonSubPath::corners(
                    [(w * 0.5, 0.0), (w, h), (0.0, h)],
                    true,
                )]),
            )
        }
    };
    let mut r = blank(id, w, h);
    r.item_transform = translate(BODY_X + x + dx, OBST_Y);
    r.fill_color = Some("Color/Paper".to_string());
    r.stroke_color = Some("Color/Black".to_string());
    r.stroke_weight_pt = Some(0.5);
    r.custom_subpaths = subpaths;
    r.text_wrap = Some(TextWrap {
        side: c.side,
        inverse: c.inverse,
        master_only: c.on_master.unwrap_or(false),
        contour_type: c.contour,
        ..TextWrap::new(c.mode, [OFFSET; 4])
    });
    r
}

/// The case's label and body frames, plus its obstacle unless the
/// obstacle lives on the master, at page-relative `dx`.
fn page_items(
    idx: usize,
    c: &Case,
    dx: f32,
    stories: &mut Vec<(String, Vec<u8>)>,
) -> Vec<PageItem> {
    let seq = idx as u32;
    let label_story = self_id(SAMPLE, "LabelStory", seq);
    stories.push((
        label_story.clone(),
        write_story(&Story {
            extra_story_attrs: Vec::new(),
            self_id: label_story.clone(),
            paragraphs: vec![Paragraph::plain(format!("text-wrap-sides · {}", c.label))],
        }),
    ));
    let body_story = self_id(SAMPLE, "BodyStory", seq);
    stories.push((
        body_story.clone(),
        write_story(&Story {
            extra_story_attrs: Vec::new(),
            self_id: body_story.clone(),
            paragraphs: body_paragraphs(),
        }),
    ));
    let label = text_frame(
        self_id(SAMPLE, "LabelFrame", seq),
        label_story,
        (BODY_W, 20.0),
        (BODY_X + dx, 72.0),
    );
    let mut body = text_frame(
        self_id(SAMPLE, "BodyFrame", seq),
        body_story,
        (BODY_W, BODY_H),
        (BODY_X + dx, BODY_Y),
    );
    if c.ignore_wrap {
        body.text_frame_pref = Some(TextFramePref {
            ignore_wrap: Some(true),
            ..TextFramePref::default()
        });
    }
    let mut items: Vec<PageItem> = vec![label.into(), body.into()];
    if c.on_master.is_none() {
        items.push(obstacle(self_id(SAMPLE, "Obstacle", seq), c, dx).into());
    }
    items
}

/// `Resources/Preferences.xml` stating a facing Letter document.
fn preferences() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Preferences",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.empty(
        "DocumentPreference",
        &[
            ("PageHeight", "792"),
            ("PageWidth", "612"),
            ("FacingPages", "true"),
            ("PageBinding", "LeftToRight"),
        ],
    );
    b.empty("TextPreference", &[("UseOpticalSize", "false")]);
    b.end("idPkg:Preferences");
    b.into_bytes()
}

pub fn build() -> Sample {
    let cases = cases();
    let mut stories = Vec::new();
    // Master A is empty; B and C carry the obstacle on both pages, C
    // with ApplyToMasterPageOnly.
    let masters: Vec<(String, Vec<u8>)> = [("A", None), ("B", Some(13usize)), ("C", Some(14))]
        .iter()
        .enumerate()
        .map(|(m, (prefix, case_no))| {
            let id = self_id(SAMPLE, "MasterSpread", m as u32);
            let items: Vec<PageItem> = match case_no {
                None => Vec::new(),
                Some(n) => {
                    let c = &cases[n - 1];
                    vec![
                        obstacle(
                            self_id(SAMPLE, "MasterObstacle", (m * 2) as u32),
                            c,
                            -PAGE_W_PT,
                        )
                        .into(),
                        obstacle(
                            self_id(SAMPLE, "MasterObstacle", (m * 2 + 1) as u32),
                            c,
                            0.0,
                        )
                        .into(),
                    ]
                }
            };
            let xml = write_facing_master(&FacingMaster {
                self_id: format!("MasterSpread/{id}"),
                name_prefix: prefix.to_string(),
                base_name: "Master".to_string(),
                verso_page_self_id: self_id(SAMPLE, "MasterPage", (m * 2) as u32),
                recto_page_self_id: self_id(SAMPLE, "MasterPage", (m * 2 + 1) as u32),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items,
            });
            (id, xml)
        })
        .collect();
    let master_of = |c: &Case| match c.on_master {
        None => format!("MasterSpread/{}", masters[0].0),
        Some(false) => format!("MasterSpread/{}", masters[1].0),
        Some(true) => format!("MasterSpread/{}", masters[2].0),
    };
    let page_id = |i: usize| self_id(SAMPLE, "Page", i as u32);

    let mut spreads = Vec::new();
    // Page 1: the recto that opens a facing document.
    let s0 = self_id(SAMPLE, "Spread", 0);
    let items = page_items(0, &cases[0], 0.0, &mut stories);
    spreads.push((
        s0.clone(),
        write_spread(&Spread {
            self_id: s0,
            page_self_id: page_id(0),
            page_name: "1".to_string(),
            applied_master: master_of(&cases[0]),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: items,
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        }),
    ));
    // Pages 2–15: reader spreads, the verso at negative x.
    for (k, pair) in (1..cases.len()).collect::<Vec<_>>().chunks(2).enumerate() {
        let (v, r) = (pair[0], pair[1]);
        let mut items = page_items(v, &cases[v], -PAGE_W_PT, &mut stories);
        items.extend(page_items(r, &cases[r], 0.0, &mut stories));
        let side = |i: usize| FacingPage {
            self_id: page_id(i),
            name: (i + 1).to_string(),
            applied_master: master_of(&cases[i]),
            override_list: Vec::new(),
            margins: None,
        };
        let id = self_id(SAMPLE, "Spread", (k + 1) as u32);
        spreads.push((
            id.clone(),
            write_facing_spread(&FacingSpread {
                self_id: id,
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                verso: side(v),
                recto: side(r),
                page_items: items,
            }),
        ));
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: masters.iter().map(|(id, _)| id.clone()).collect(),
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: masters,
        spreads,
        stories,
    }
}
