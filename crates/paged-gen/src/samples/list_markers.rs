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

//! `list-markers.idml` — where the text after a bullet or a number
//! starts, against InDesign.
//!
//! Each case is one frame holding two list paragraphs whose text begins
//! with the case's tag (`c03 one`, `c03 two`), so the x of the word after
//! each marker reads straight off `pdftotext -bbox`. The cases vary what
//! follows the marker (`BulletsTextAfter` undeclared / `^t` / a space;
//! `NumberingExpression` `^#.^t` / `^#.`), the indents (none, left only,
//! hanging, hanging narrower than the marker) and the tab stops (none,
//! one before the left indent, one beyond it, one under the marker), plus
//! a plain tab in a hanging-indent paragraph with no list at all.
//!
//! Frames have zero insets, so a frame-local x is a tab-stop position.
//! Text is Inter 10/12 (registered by `list-markers.fonts.sh`).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story, TabStop},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "list-markers";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
pub const FRAME_W: f32 = 200.0;
const FRAME_H: f32 = 56.0;

/// `BulletsTextAfter="^t"`.
pub const STYLE_AFTER_TAB: &str = "ParagraphStyle/AfterTab";
/// `BulletsTextAfter=" "`.
pub const STYLE_AFTER_SPACE: &str = "ParagraphStyle/AfterSpace";
/// `NumberingExpression="^#."` — no tab after the number.
pub const STYLE_NUMBER_NO_TAB: &str = "ParagraphStyle/NumberNoTab";

/// One case: two paragraphs `"<tag> one"`, `"<tag> two"` sharing the
/// list, indents, style and tab stops. `prefix` goes before the tag
/// (the plain-tab case's `"Tab\t"`).
pub struct Case {
    pub name: &'static str,
    pub tag: &'static str,
    pub list: Option<&'static str>,
    pub style: Option<&'static str>,
    pub left_indent: f32,
    pub first_line_indent: f32,
    pub tab_stops: &'static [f32],
    pub prefix: &'static str,
}

const fn case(name: &'static str, tag: &'static str) -> Case {
    Case {
        name,
        tag,
        list: Some("BulletList"),
        style: None,
        left_indent: 18.0,
        first_line_indent: -18.0,
        tab_stops: &[],
        prefix: "",
    }
}

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    vec![
        case("bullet · after undeclared · hanging 18", "c00"),
        Case {
            style: Some(STYLE_AFTER_TAB),
            ..case("bullet · after ^t · hanging 18", "c01")
        },
        Case {
            style: Some(STYLE_AFTER_SPACE),
            ..case("bullet · after space · hanging 18", "c02")
        },
        Case {
            style: Some(STYLE_AFTER_TAB),
            left_indent: 0.0,
            first_line_indent: 0.0,
            ..case("bullet · ^t · no indent", "c03")
        },
        Case {
            style: Some(STYLE_AFTER_TAB),
            first_line_indent: 0.0,
            ..case("bullet · ^t · left indent 18 only", "c04")
        },
        Case {
            style: Some(STYLE_AFTER_TAB),
            left_indent: 50.0,
            first_line_indent: -20.0,
            ..case("bullet · ^t · left 50 first -20", "c05")
        },
        Case {
            style: Some(STYLE_AFTER_TAB),
            left_indent: 50.0,
            first_line_indent: -50.0,
            tab_stops: &[30.0],
            ..case("bullet · ^t · hanging 50 · stop 30", "c06")
        },
        Case {
            style: Some(STYLE_AFTER_TAB),
            left_indent: 30.0,
            first_line_indent: -30.0,
            tab_stops: &[60.0],
            ..case("bullet · ^t · hanging 30 · stop 60", "c07")
        },
        Case {
            list: Some("NumberedList"),
            ..case("number · ^#.^t · hanging 18", "c08")
        },
        Case {
            list: Some("NumberedList"),
            style: Some(STYLE_NUMBER_NO_TAB),
            ..case("number · ^#. · hanging 18", "c09")
        },
        Case {
            list: Some("NumberedList"),
            left_indent: 50.0,
            first_line_indent: -50.0,
            tab_stops: &[10.0],
            ..case("number · ^#.^t · hanging 50 · stop 10", "c10")
        },
        Case {
            list: Some("NumberedList"),
            left_indent: 6.0,
            first_line_indent: -6.0,
            ..case("number · ^#.^t · hanging 6", "c11")
        },
        Case {
            list: None,
            left_indent: 40.0,
            first_line_indent: -40.0,
            prefix: "Tab\t",
            ..case("no list · plain tab · hanging 40", "c12")
        },
        Case {
            style: Some(STYLE_AFTER_TAB),
            tab_stops: &[2.0],
            ..case("bullet · ^t · hanging 18 · stop 2", "c13")
        },
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// Top-left of case `i`'s body frame, page-local pt.
pub fn frame_origin(i: u32) -> (f32, f32) {
    let col = i % 2;
    let row = i / 2;
    (72.0 + col as f32 * 240.0, 72.0 + row as f32 * 100.0)
}

/// The paragraph styles the cases apply, each a one-attribute variation
/// on `[No paragraph style]`.
fn styles_fragment() -> String {
    let style = |self_id: &str, name: &str, attr: &str| {
        format!(
            "<ParagraphStyle Self=\"{self_id}\" Name=\"{name}\" {attr}>\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn></Properties>\
</ParagraphStyle>"
        )
    };
    format!(
        "<RootParagraphStyleGroup>{}{}{}</RootParagraphStyleGroup>",
        style(STYLE_AFTER_TAB, "AfterTab", "BulletsTextAfter=\"^t\""),
        style(STYLE_AFTER_SPACE, "AfterSpace", "BulletsTextAfter=\" \""),
        style(
            STYLE_NUMBER_NO_TAB,
            "NumberNoTab",
            "NumberingExpression=\"^#.\""
        ),
    )
}

fn paragraph(c: &Case, word: &str) -> Paragraph {
    let mut attrs = vec![("Hyphenation", "false")];
    if let Some(s) = c.style {
        attrs.push(("AppliedParagraphStyle", s));
    }
    Paragraph {
        extra_paragraph_attrs: attrs,
        leading: Some(LEADING),
        first_line_indent: Some(c.first_line_indent),
        left_indent: Some(c.left_indent),
        bullets_list_type: c.list,
        tab_list: c
            .tab_stops
            .iter()
            .map(|&position_pt| TabStop {
                position_pt,
                alignment: "LeftAlign",
                leader: None,
            })
            .collect(),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: format!("{}{} {word}", c.prefix, c.tag),
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
        graphic_xml: graphic_xml(),
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
