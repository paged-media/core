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

//! `inline-objects.idml` — how an inline (`InlinePosition`) and an
//! above-line (`AboveLine`) anchored object take room in their story.
//!
//! Every page is one case, asked of InDesign: stroked boxes of several
//! heights anchored at a line's start, middle and end, in auto-leaded and
//! fixed-leaded paragraphs, with a Y offset, taller than their frame, as
//! the frame's first line, and above-line objects with each alignment and
//! a space after. Body text is numbered tokens (`b07`) in Inter 10 pt, so
//! a moved line break reads at a glance. The anchors sit at the START of
//! the run that carries them (the builder writes the object before the
//! run's text), which is the character offset the pipeline test pins.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{AnchoredObjectSetting, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "inline-objects";
pub const PAGE_W_PT: f32 = 432.0;
pub const PAGE_H_PT: f32 = 432.0;
pub const BODY_X_PT: f32 = 36.0;
pub const BODY_Y_PT: f32 = 60.0;
pub const BODY_W_PT: f32 = 360.0;
pub const BODY_H_PT: f32 = 340.0;
const BODY_FONT: &str = "Inter";
pub const POINT_SIZE: f32 = 10.0;
/// The fixed leading of the fixed-leaded cases.
pub const FIXED_LEADING: f32 = 12.0;

/// One piece of a paragraph: numbered tokens, or an anchored box.
pub enum Piece {
    /// `n` tokens, continuing the paragraph's numbering.
    Words(u32),
    /// A `w × h` box with this anchored setting.
    Object(f32, f32, AnchoredObjectSetting),
}

/// One paragraph of a case: its token prefix and its pieces.
pub struct Para {
    pub prefix: char,
    pub pieces: Vec<Piece>,
}

/// One text frame of a case.
pub struct FrameCase {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `Some(pt)` sets every run's `Leading`; `None` is auto leading.
    pub leading: Option<f32>,
    pub paragraphs: Vec<Para>,
}

/// One page: its label and its frames.
pub struct Case {
    pub name: &'static str,
    pub frames: Vec<FrameCase>,
}

fn inline() -> AnchoredObjectSetting {
    AnchoredObjectSetting::inline()
}

fn inline_y(offset: f32) -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchor_y_offset: Some(offset),
        ..AnchoredObjectSetting::inline()
    }
}

fn above(alignment: &'static str, space_after: f32) -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "AboveLine",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("TopLeftAnchor"),
        horizontal_reference_point: Some("ColumnEdge"),
        vertical_reference_point: Some("LineBaseline"),
        horizontal_alignment: Some(alignment),
        vertical_alignment: Some("TopAlign"),
        anchor_x_offset: None,
        anchor_y_offset: Some(space_after),
    }
}

fn body(leading: Option<f32>, paragraphs: Vec<Para>) -> FrameCase {
    FrameCase {
        x: BODY_X_PT,
        y: BODY_Y_PT,
        w: BODY_W_PT,
        h: BODY_H_PT,
        leading,
        paragraphs,
    }
}

fn p(prefix: char, pieces: Vec<Piece>) -> Para {
    Para { prefix, pieces }
}

use Piece::{Object as O, Words as W};

/// The inline cases of pages 1 (auto) and 2 (fixed leading): a short
/// box at a line start, a 24 pt box mid first line, a 48 pt box at the
/// paragraph's end, a 24 pt box mid second line, then a plain paragraph.
fn inline_heights() -> Vec<Para> {
    vec![
        p('a', vec![O(30.0, 6.0, inline()), W(30)]),
        p('b', vec![W(6), O(30.0, 24.0, inline()), W(24)]),
        p('c', vec![W(30), O(30.0, 48.0, inline())]),
        p('d', vec![W(18), O(30.0, 24.0, inline()), W(12)]),
        p('e', vec![W(20)]),
    ]
}

pub fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "inline · auto leading · 6/24/48 pt at start/middle/end",
            frames: vec![body(None, inline_heights())],
        },
        Case {
            name: "inline · fixed 12 pt leading · 6/24/48 pt at start/middle/end",
            frames: vec![body(Some(FIXED_LEADING), inline_heights())],
        },
        Case {
            name: "inline · AnchorYoffset +6 / -6 · auto and fixed",
            frames: vec![
                body(
                    None,
                    vec![
                        p('a', vec![W(10)]),
                        p('b', vec![O(30.0, 24.0, inline_y(6.0)), W(20)]),
                        p('c', vec![W(6), O(30.0, 24.0, inline_y(-6.0)), W(20)]),
                        p('d', vec![W(20)]),
                    ],
                ),
                FrameCase {
                    y: 240.0,
                    h: 150.0,
                    ..body(
                        Some(FIXED_LEADING),
                        vec![
                            p('e', vec![W(10)]),
                            p('f', vec![O(30.0, 24.0, inline_y(6.0)), W(20)]),
                            p('g', vec![W(20)]),
                        ],
                    )
                },
            ],
        },
        Case {
            name: "inline · first line of the frame · auto and fixed",
            frames: vec![
                FrameCase {
                    h: 140.0,
                    ..body(
                        None,
                        vec![
                            p('a', vec![O(30.0, 36.0, inline()), W(20)]),
                            p('b', vec![W(10)]),
                        ],
                    )
                },
                FrameCase {
                    y: 240.0,
                    h: 140.0,
                    ..body(
                        Some(FIXED_LEADING),
                        vec![
                            p('c', vec![O(30.0, 36.0, inline()), W(20)]),
                            p('d', vec![W(10)]),
                        ],
                    )
                },
            ],
        },
        Case {
            name: "inline · taller than its frame",
            frames: vec![
                FrameCase {
                    h: 100.0,
                    ..body(
                        None,
                        vec![
                            p('a', vec![W(10)]),
                            p('b', vec![W(4), O(30.0, 150.0, inline()), W(10)]),
                        ],
                    )
                },
                FrameCase {
                    y: 240.0,
                    h: 100.0,
                    ..body(None, vec![p('c', vec![O(30.0, 150.0, inline()), W(10)])])
                },
            ],
        },
        Case {
            name: "above line · left / center / right · space after 6",
            frames: vec![body(
                None,
                vec![
                    p(
                        'a',
                        vec![W(6), O(80.0, 24.0, above("LeftAlign", 0.0)), W(20)],
                    ),
                    p(
                        'b',
                        vec![W(6), O(80.0, 24.0, above("CenterAlign", 0.0)), W(20)],
                    ),
                    p(
                        'c',
                        vec![W(6), O(80.0, 24.0, above("RightAlign", 0.0)), W(20)],
                    ),
                    p(
                        'd',
                        vec![W(18), O(80.0, 24.0, above("LeftAlign", 6.0)), W(10)],
                    ),
                    p('e', vec![W(10)]),
                ],
            )],
        },
        Case {
            name: "above line · first line of the frame · fixed leading",
            frames: vec![
                FrameCase {
                    h: 140.0,
                    ..body(
                        None,
                        vec![
                            p('a', vec![O(80.0, 24.0, above("LeftAlign", 0.0)), W(20)]),
                            p('b', vec![W(10)]),
                        ],
                    )
                },
                FrameCase {
                    y: 240.0,
                    h: 140.0,
                    ..body(
                        Some(FIXED_LEADING),
                        vec![
                            p(
                                'c',
                                vec![W(4), O(80.0, 24.0, above("CenterAlign", 6.0)), W(20)],
                            ),
                            p('d', vec![W(10)]),
                        ],
                    )
                },
            ],
        },
    ]
}

fn run(text: String, leading: Option<&'static str>, frame: Option<Rect>) -> Run {
    Run {
        extra_char_attrs: leading.map(|l| vec![("Leading", l)]).unwrap_or_default(),
        text,
        point_size: Some(POINT_SIZE),
        fill_color: Some("Color/Black".to_string()),
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: Some(BODY_FONT),
        anchored_frame: frame,
    }
}

fn plain_rect(self_id: String, w: f32, h: f32, at: (f32, f32), story: Option<String>) -> Rect {
    Rect {
        self_id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: story,
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

/// The token text of `n` words of paragraph `prefix`, numbered on from
/// `*next`; a trailing space when more pieces follow.
fn words(prefix: char, next: &mut u32, n: u32, trailing_space: bool) -> String {
    let mut s = (0..n)
        .map(|i| format!("{prefix}{:02}", *next + i))
        .collect::<Vec<_>>()
        .join(" ");
    *next += n;
    if trailing_space && !s.is_empty() {
        s.push(' ');
    }
    s
}

pub fn build() -> Sample {
    let cases = cases();
    let mut master_spreads = Vec::new();
    let mut spreads = Vec::new();
    let mut stories = Vec::new();
    let mut master_refs = Vec::new();
    let mut spread_refs = Vec::new();
    let mut story_refs = Vec::new();
    let mut object_seq = 0u32;
    let mut frame_seq = 0u32;

    for (i, case) in cases.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
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

        let label_story = self_id(SAMPLE, "LabelStory", seq);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph::plain(case.name)],
            }),
        ));
        story_refs.push(label_story.clone());
        let mut items = vec![plain_rect(
            self_id(SAMPLE, "LabelFrame", seq),
            BODY_W_PT,
            20.0,
            (BODY_X_PT, 24.0),
            Some(label_story),
        )
        .into()];

        for frame in &case.frames {
            let story_id = self_id(SAMPLE, "BodyStory", frame_seq);
            let frame_id = self_id(SAMPLE, "BodyFrame", frame_seq);
            frame_seq += 1;
            let leading = frame.leading.map(|l| -> &'static str {
                if (l - FIXED_LEADING).abs() < 1e-3 {
                    "12"
                } else {
                    unreachable!("one fixed leading in this fixture")
                }
            });
            let paragraphs = frame
                .paragraphs
                .iter()
                .map(|para| {
                    let mut next = 1u32;
                    let mut runs = Vec::new();
                    let mut pending: Option<Rect> = None;
                    for (k, piece) in para.pieces.iter().enumerate() {
                        let more = k + 1 < para.pieces.len();
                        match piece {
                            Piece::Words(n) => {
                                let mut text = words(para.prefix, &mut next, *n, more);
                                // An object is a word of its own: a space
                                // on both sides of it.
                                if pending.is_some() {
                                    text.insert(0, ' ');
                                }
                                runs.push(run(text, leading, pending.take()));
                            }
                            Piece::Object(w, h, setting) => {
                                if let Some(prev) = pending.take() {
                                    runs.push(run(String::new(), leading, Some(prev)));
                                }
                                let id = self_id(SAMPLE, "Object", object_seq);
                                object_seq += 1;
                                pending = Some(Rect {
                                    fill_color: None,
                                    stroke_color: Some("Color/Black".to_string()),
                                    stroke_weight_pt: Some(0.5),
                                    item_transform: IDENTITY,
                                    anchored_setting: Some(setting.clone()),
                                    ..plain_rect(id, *w, *h, (0.0, 0.0), None)
                                });
                            }
                        }
                    }
                    if let Some(prev) = pending.take() {
                        runs.push(run(String::new(), leading, Some(prev)));
                    }
                    Paragraph {
                        extra_paragraph_attrs: vec![("Hyphenation", "false")],
                        runs,
                        ..Paragraph::plain("")
                    }
                })
                .collect();
            stories.push((
                story_id.clone(),
                write_story(&Story {
                    extra_story_attrs: Vec::new(),
                    self_id: story_id.clone(),
                    paragraphs,
                }),
            ));
            story_refs.push(story_id.clone());
            items.push(
                plain_rect(
                    frame_id,
                    frame.w,
                    frame.h,
                    (frame.x, frame.y),
                    Some(story_id),
                )
                .into(),
            );
        }

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: case.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items,
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
            master_spreads: master_refs,
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
        master_spreads,
        spreads,
        stories,
    }
}
