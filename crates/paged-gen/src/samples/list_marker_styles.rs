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

//! `list-marker-styles` — what a list marker's character style does to
//! the marker, asked of InDesign.
//!
//! `list-overrides` binds `BulletsCharacterStyle` /
//! `NumberingCharacterStyle` (o14/o16) to one 20 pt style. This sample
//! gives the marker a character style per attribute — size, family,
//! style (bold), colour, baseline shift, tracking, and one that sets
//! nothing — each for a bullet and a number, with a space separator so
//! the marker's advance reads straight off the word after it. Plus: a
//! big marker under auto leading (does it raise the line?), a big marker
//! before a tab, the style bound by the PARAGRAPH style, and a run whose
//! own formatting the marker may or may not inherit.
//!
//! Same layout contract as `list-overrides`: one frame per case, two
//! list paragraphs `"<tag> one"` / `"<tag> two"`, zero insets, Inter 10.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml_with_raw,
        ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "list-marker-styles";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
pub const FRAME_W: f32 = 150.0;
const FRAME_H: f32 = 56.0;

/// `PointSize="20"`.
pub const CHAR_BIG: &str = "CharacterStyle/Big";
/// `FontStyle="Bold"` (Inter Bold).
pub const CHAR_BOLD: &str = "CharacterStyle/Bold";
/// `AppliedFont` JetBrains Mono, `FontStyle="Regular"`.
pub const CHAR_MONO: &str = "CharacterStyle/Mono";
/// `FillColor="Color/Red"`.
pub const CHAR_RED: &str = "CharacterStyle/Red";
/// `BaselineShift="4"`.
pub const CHAR_SHIFT: &str = "CharacterStyle/Shift";
/// `Tracking="200"`.
pub const CHAR_TRACK: &str = "CharacterStyle/Track";
/// Declares nothing.
pub const CHAR_EMPTY: &str = "CharacterStyle/Empty";
/// A numbered paragraph style whose `NumberingCharacterStyle` is
/// [`CHAR_BIG`].
pub const STYLE_NUMBER_BIG: &str = "ParagraphStyle/NumberBig";

/// One case: two paragraphs `"<tag> one"`, `"<tag> two"`.
pub struct Case {
    pub name: &'static str,
    pub tag: &'static str,
    pub list: &'static str,
    pub style: Option<&'static str>,
    /// The marker's character style, as the typed `<Properties>` child
    /// InDesign reads (`BulletsCharacterStyle` for a bullet,
    /// `NumberingCharacterStyle` for a number).
    pub marker_style: Option<&'static str>,
    /// `BulletsTextAfter` (bullet) / `NumberingExpression` (number).
    pub after: &'static str,
    /// `None` = auto leading.
    pub leading: Option<f32>,
    pub left_indent: f32,
    pub first_line_indent: f32,
    /// The run's own `FontStyle` / `FillColor`.
    pub run_font_style: Option<&'static str>,
    pub run_fill: Option<&'static str>,
}

const fn bullet(name: &'static str, tag: &'static str, marker_style: &'static str) -> Case {
    Case {
        name,
        tag,
        list: "BulletList",
        style: None,
        marker_style: Some(marker_style),
        after: " ",
        leading: Some(LEADING),
        left_indent: 18.0,
        first_line_indent: -18.0,
        run_font_style: None,
        run_fill: None,
    }
}

const fn number(name: &'static str, tag: &'static str, marker_style: &'static str) -> Case {
    Case {
        list: "NumberedList",
        after: "^#. ",
        ..bullet(name, tag, marker_style)
    }
}

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    vec![
        bullet("bullet: size 20", "m00", CHAR_BIG),
        number("number: size 20", "m01", CHAR_BIG),
        bullet("bullet: bold", "m02", CHAR_BOLD),
        number("number: bold", "m03", CHAR_BOLD),
        bullet("bullet: JetBrains Mono", "m04", CHAR_MONO),
        number("number: JetBrains Mono", "m05", CHAR_MONO),
        bullet("bullet: red", "m06", CHAR_RED),
        number("number: red", "m07", CHAR_RED),
        bullet("bullet: baseline shift 4", "m08", CHAR_SHIFT),
        number("number: baseline shift 4", "m09", CHAR_SHIFT),
        bullet("bullet: tracking 200", "m10", CHAR_TRACK),
        number("number: tracking 200", "m11", CHAR_TRACK),
        bullet("bullet: empty style", "m12", CHAR_EMPTY),
        number("number: empty style", "m13", CHAR_EMPTY),
        Case {
            leading: None,
            ..bullet("bullet: size 20, auto leading", "m14", CHAR_BIG)
        },
        Case {
            leading: None,
            ..number("number: size 20, auto leading", "m15", CHAR_BIG)
        },
        Case {
            after: "^t",
            ..bullet("bullet: size 20, then ^t", "m16", CHAR_BIG)
        },
        Case {
            after: "^#.^t",
            left_indent: 30.0,
            first_line_indent: -30.0,
            ..number("number: size 20, ^#.^t hang 30", "m17", CHAR_BIG)
        },
        Case {
            style: Some(STYLE_NUMBER_BIG),
            marker_style: None,
            ..number("number: size 20 from para style", "m18", CHAR_BIG)
        },
        Case {
            marker_style: None,
            run_font_style: Some("Bold"),
            run_fill: Some("Color/Red"),
            ..bullet("bullet: no style, run bold red", "m19", CHAR_BIG)
        },
        Case {
            run_font_style: Some("Bold"),
            ..number("number: size 20, run bold", "m20", CHAR_BIG)
        },
        Case {
            run_fill: Some("Color/Red"),
            ..number("number: bold, run red", "m21", CHAR_BOLD)
        },
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// Top-left of case `i`'s body frame, page-local pt.
pub fn frame_origin(i: u32) -> (f32, f32) {
    let col = i % 3;
    let row = i / 3;
    (36.0 + col as f32 * 180.0, 72.0 + row as f32 * 80.0)
}

fn styles_fragment() -> String {
    let ch = |self_id: &str, name: &str, attr: &str, props: &str| {
        format!(
            "<CharacterStyle Self=\"{self_id}\" Name=\"{name}\" {attr}>\
<Properties><BasedOn type=\"string\">$ID/[No character style]</BasedOn>{props}</Properties>\
</CharacterStyle>"
        )
    };
    format!(
        "<RootCharacterStyleGroup>{}{}{}{}{}{}{}</RootCharacterStyleGroup>\
<RootParagraphStyleGroup><ParagraphStyle Self=\"{STYLE_NUMBER_BIG}\" Name=\"NumberBig\">\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn>\
<NumberingCharacterStyle type=\"object\">{CHAR_BIG}</NumberingCharacterStyle></Properties>\
</ParagraphStyle></RootParagraphStyleGroup>",
        ch(CHAR_BIG, "Big", "PointSize=\"20\"", ""),
        ch(CHAR_BOLD, "Bold", "FontStyle=\"Bold\"", ""),
        ch(
            CHAR_MONO,
            "Mono",
            "FontStyle=\"Regular\"",
            "<AppliedFont type=\"string\">JetBrains Mono</AppliedFont>"
        ),
        ch(CHAR_RED, "Red", "FillColor=\"Color/Red\"", ""),
        ch(CHAR_SHIFT, "Shift", "BaselineShift=\"4\"", ""),
        ch(CHAR_TRACK, "Track", "Tracking=\"200\"", ""),
        ch(CHAR_EMPTY, "Empty", "", ""),
    )
}

fn paragraph(c: &Case, word: &str) -> Paragraph {
    let mut attrs = vec![("Hyphenation", "false")];
    if let Some(s) = c.style {
        attrs.push(("AppliedParagraphStyle", s));
    }
    let mut props = Vec::new();
    let numbered = c.list == "NumberedList";
    props.push(if numbered {
        ("NumberingExpression", "string", c.after)
    } else {
        ("BulletsTextAfter", "string", c.after)
    });
    if let Some(m) = c.marker_style {
        props.push(if numbered {
            ("NumberingCharacterStyle", "object", m)
        } else {
            ("BulletsCharacterStyle", "object", m)
        });
    }
    Paragraph {
        extra_paragraph_attrs: attrs,
        extra_paragraph_props: props,
        leading: c.leading,
        first_line_indent: Some(c.first_line_indent),
        left_indent: Some(c.left_indent),
        bullets_list_type: Some(c.list),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: format!("{} {word}", c.tag),
            point_size: Some(POINT_SIZE),
            fill_color: Some(c.run_fill.unwrap_or("Color/Black").to_string()),
            font_style: c.run_font_style,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

fn frame(id: String, story: String, w: f32, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        stroke_color: body.then(|| "Color/Black".to_string()),
        stroke_weight_pt: body.then_some(0.25),
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
        text_frame_pref: body.then(|| TextFramePref {
            inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
            first_baseline_offset: Some("LeadingOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items = Vec::new();

    for (i, case) in cases().iter().enumerate() {
        let seq = i as u32;
        let (x, y) = frame_origin(seq);
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
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label_story,
            FRAME_W,
            20.0,
            (x, y - 24.0),
            false,
        ));

        let body = body_story_id(seq);
        stories.push((
            body.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body.clone(),
                paragraphs: vec![paragraph(case, "one"), paragraph(case, "two")],
            }),
        ));
        story_refs.push(body.clone());
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            FRAME_W,
            FRAME_H,
            (x, y),
            true,
        ));
    }

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: self_id(SAMPLE, "Page", 0),
        page_name: "1".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: items,
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_with_extras(&[ExtraColor {
            self_id: "Color/Red".to_string(),
            name: "Red".to_string(),
            space: "RGB",
            value: "230 0 0".to_string(),
        }]),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml_with_raw(&styles_fragment()),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
