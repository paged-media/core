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

//! `stroke-inset.idml` — how a text frame's STROKE moves its text,
//! asked of InDesign.
//!
//! Every case is one frame of the `keeps` line grid (Inter 10/12, a
//! `LeadingOffset` first baseline, hyphenation off) holding short
//! one-line paragraphs that alternate left- and right-aligned, so each
//! frame shows where its text area's left edge, right edge, first
//! baseline and bottom are. The cases vary the stroke weight (0.25 / 1 /
//! 6 / 12 pt), its `StrokeAlignment` (centre / inside / outside), an
//! invisible stroke (weight without a colour), `InsetSpacing` on top of
//! a stroke, frames whose height lets the stroke decide whether the
//! fifth line fits, two-column frames, and two non-rectangular outlines
//! (rounded corners, a chamfered polygon).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, PolygonSubPath, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "stroke-inset";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
pub const FRAME_W: f32 = 200.0;
/// Five 12 pt lines fit with no stroke (baseline 60 inside 66).
pub const FRAME_H: f32 = 66.0;
/// Cases per page: two columns of eight rows.
pub const PER_PAGE: usize = 16;

/// The outline a case's frame carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Rect,
    /// `CornerOption="RoundedCorner"` with this radius.
    Rounded(u32),
    /// A rectangle with its top-left corner cut off by a 45° chamfer
    /// this many points deep (a five-point polygon).
    Chamfer(u32),
}

pub struct Case {
    pub name: &'static str,
    /// `StrokeWeight`; `0` writes no stroke.
    pub weight: f32,
    /// Whether the stroke has a colour (`Color/Black`) or is
    /// `Swatch/None` at that weight.
    pub visible: bool,
    /// `StrokeAlignment`, `None` ⇒ the attribute is omitted.
    pub alignment: Option<&'static str>,
    /// `InsetSpacing` `[top, left, bottom, right]`.
    pub inset: [f32; 4],
    pub height: f32,
    pub columns: u32,
    pub shape: Shape,
    /// One-line paragraphs; odd ones left-aligned, even ones right.
    pub paragraphs: usize,
}

const fn case(name: &'static str, weight: f32, alignment: Option<&'static str>) -> Case {
    Case {
        name,
        weight,
        visible: true,
        alignment,
        inset: [0.0; 4],
        height: FRAME_H,
        columns: 1,
        shape: Shape::Rect,
        paragraphs: 6,
    }
}

const CENTER: Option<&str> = Some("CenterAlignment");
const INSIDE: Option<&str> = Some("InsideAlignment");
const OUTSIDE: Option<&str> = Some("OutsideAlignment");

/// The column gutter of the two-column cases.
pub const GUTTER: f32 = 20.0;

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    let fit = |name, weight, alignment| Case {
        height: 60.3,
        ..case(name, weight, alignment)
    };
    let columns = |name, alignment| Case {
        height: 42.0,
        columns: 2,
        paragraphs: 8,
        ..case(name, 6.0, alignment)
    };
    vec![
        case("0.25 centre (attribute omitted)", 0.25, None),
        case("0.25 inside", 0.25, INSIDE),
        case("0.25 outside", 0.25, OUTSIDE),
        case("1 centre", 1.0, CENTER),
        case("1 inside", 1.0, INSIDE),
        case("1 outside", 1.0, OUTSIDE),
        case("6 centre", 6.0, CENTER),
        case("6 inside", 6.0, INSIDE),
        case("6 outside", 6.0, OUTSIDE),
        case("12 centre", 12.0, CENTER),
        case("12 inside", 12.0, INSIDE),
        case("12 outside", 12.0, OUTSIDE),
        case("no stroke", 0.0, None),
        Case {
            visible: false,
            ..case("6 centre, no colour", 6.0, CENTER)
        },
        Case {
            visible: false,
            ..case("6 inside, no colour", 6.0, INSIDE)
        },
        Case {
            inset: [4.0; 4],
            ..case("inset 4 + 6 centre", 6.0, CENTER)
        },
        Case {
            inset: [4.0; 4],
            ..case("inset 4 + 6 inside", 6.0, INSIDE)
        },
        Case {
            inset: [4.0; 4],
            ..case("inset 4 + 6 outside", 6.0, OUTSIDE)
        },
        Case {
            inset: [2.0, 8.0, 6.0, 10.0],
            ..case("inset 2/8/6/10 + 1 centre", 1.0, CENTER)
        },
        fit("fit 60.3, no stroke", 0.0, None),
        fit("fit 60.3, 0.25 centre", 0.25, CENTER),
        fit("fit 60.3, 1 centre", 1.0, CENTER),
        fit("fit 60.3, 0.25 inside", 0.25, INSIDE),
        columns("2 columns, 6 centre", CENTER),
        columns("2 columns, 6 inside", INSIDE),
        Case {
            shape: Shape::Rounded(12),
            ..case("rounded 12, 6 centre", 6.0, CENTER)
        },
        Case {
            shape: Shape::Rounded(12),
            ..case("rounded 12, no stroke", 0.0, None)
        },
        Case {
            shape: Shape::Chamfer(30),
            ..case("chamfer 30, 6 centre", 6.0, CENTER)
        },
        Case {
            shape: Shape::Chamfer(30),
            ..case("chamfer 30, no stroke", 0.0, None)
        },
        Case {
            shape: Shape::Chamfer(30),
            inset: [4.0; 4],
            ..case("chamfer 30, inset 4 + 6 centre", 6.0, CENTER)
        },
        Case {
            shape: Shape::Chamfer(30),
            inset: [4.0; 4],
            ..case("chamfer 30, inset 4, no stroke", 0.0, None)
        },
    ]
}

/// The text of paragraph `k` (0-based) of a case.
pub fn paragraph_text(k: usize) -> String {
    const WORDS: [&str; 8] = [
        "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight",
    ];
    let side = if k % 2 == 0 { "left" } else { "right" };
    format!("{} {side}", WORDS[k % WORDS.len()])
}

/// Whether paragraph `k` is right-aligned.
pub fn is_right(k: usize) -> bool {
    k % 2 == 1
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
    let col = k % 2;
    let row = k / 2;
    (72.0 + col as f32 * 250.0, 60.0 + row as f32 * 96.0)
}

fn paragraph(k: usize) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(LEADING),
        justification: Some(if is_right(k) {
            "RightAlign"
        } else {
            "LeftAlign"
        }),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: paragraph_text(k),
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

fn label(i: usize, story: String, at: (f32, f32)) -> PageItem {
    Rect {
        self_id: self_id(SAMPLE, "LabelFrame", i as u32),
        width_pt: FRAME_W + 40.0,
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
    let mut extra_attrs = Vec::new();
    if let Some(a) = c.alignment {
        extra_attrs.push(("StrokeAlignment".to_string(), a.to_string()));
    }
    if let Shape::Rounded(r) = c.shape {
        // InDesign reads the per-corner spelling (the legacy uniform
        // `CornerOption` alone left the corners square in its DOM).
        extra_attrs.push(("CornerOption".to_string(), "RoundedCorner".to_string()));
        extra_attrs.push(("CornerRadius".to_string(), r.to_string()));
        for corner in ["TopLeft", "TopRight", "BottomLeft", "BottomRight"] {
            extra_attrs.push((format!("{corner}CornerOption"), "RoundedCorner".to_string()));
            extra_attrs.push((format!("{corner}CornerRadius"), r.to_string()));
        }
    }
    let custom_subpaths = match c.shape {
        Shape::Chamfer(d) => {
            let (w, h, d) = (FRAME_W, c.height, d as f32);
            Some(vec![PolygonSubPath::corners(
                [(d, 0.0), (w, 0.0), (w, h), (0.0, h), (0.0, d)],
                true,
            )])
        }
        _ => None,
    };
    Rect {
        self_id: self_id(SAMPLE, "Frame", i as u32),
        width_pt: FRAME_W,
        height_pt: c.height,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        stroke_color: (c.weight > 0.0 && c.visible).then(|| "Color/Black".to_string()),
        stroke_weight_pt: (c.weight > 0.0).then_some(c.weight),
        parent_story: Some(story),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs,
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: Some(TextFramePref {
            inset_spacing: Some(c.inset),
            first_baseline_offset: Some("LeadingOffset"),
            text_column_count: (c.columns > 1).then_some(c.columns),
            text_column_gutter: (c.columns > 1).then_some(GUTTER),
            ..Default::default()
        }),
        custom_subpaths,
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
                paragraphs: (0..c.paragraphs).map(paragraph).collect(),
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
