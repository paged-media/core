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

//! `list-overrides` — the list-marker attributes spelled as LOCAL
//! paragraph overrides, asked of InDesign.
//!
//! `list-markers` declares `BulletsTextAfter` / `NumberingExpression` in
//! paragraph styles. This sample spells the same values directly on the
//! `<ParagraphStyleRange>` (no style, and over a style that says
//! otherwise), plus the counter (`NumberingStartAt` / `NumberingContinue`),
//! the format (`NumberingFormat`) and the marker character styles
//! (`BulletsCharacterStyle` / `NumberingCharacterStyle`), each in the
//! spelling InDesign writes AND in the other one, so the reference says
//! which spelling it reads.
//!
//! Same layout contract as `list-markers`: one frame per case, two list
//! paragraphs `"<tag> one"` / `"<tag> two"`, zero insets, Inter 10/12.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "list-overrides";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
pub const FRAME_W: f32 = 150.0;
const FRAME_H: f32 = 56.0;

/// `BulletsTextAfter="^t"` (as in `list-markers`).
pub const STYLE_AFTER_TAB: &str = "ParagraphStyle/AfterTab";
/// `BulletsTextAfter=" "`.
pub const STYLE_AFTER_SPACE: &str = "ParagraphStyle/AfterSpace";
/// `NumberingExpression="^#."`.
pub const STYLE_NUMBER_NO_TAB: &str = "ParagraphStyle/NumberNoTab";
/// `NumberingStartAt="1" NumberingContinue="true"` — what InDesign's own
/// `[No paragraph style]` declares in every package it writes.
pub const STYLE_START1_CONTINUE: &str = "ParagraphStyle/Start1Continue";
/// `NumberingStartAt="5"`, `NumberingContinue` undeclared.
pub const STYLE_START5: &str = "ParagraphStyle/Start5";
/// A 20 pt character style for the list marker.
pub const CHAR_BIG: &str = "CharacterStyle/Big";

type Attrs = &'static [(&'static str, &'static str)];
type Props = &'static [(&'static str, &'static str, &'static str)];

/// One case: two paragraphs `"<tag> one"`, `"<tag> two"`. `attrs` and
/// `props` go on both ranges; `first` / `second` only on that one.
pub struct Case {
    pub name: &'static str,
    pub tag: &'static str,
    pub list: &'static str,
    pub style: Option<&'static str>,
    pub left_indent: f32,
    pub first_line_indent: f32,
    pub attrs: Attrs,
    pub props: Props,
    pub first: Attrs,
    pub second: Attrs,
}

const fn bullet(name: &'static str, tag: &'static str) -> Case {
    Case {
        name,
        tag,
        list: "BulletList",
        style: None,
        left_indent: 18.0,
        first_line_indent: -18.0,
        attrs: &[],
        props: &[],
        first: &[],
        second: &[],
    }
}

const fn number(name: &'static str, tag: &'static str) -> Case {
    Case {
        list: "NumberedList",
        ..bullet(name, tag)
    }
}

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    vec![
        Case {
            attrs: &[("BulletsTextAfter", "^t")],
            ..bullet("local after ^t (= c01)", "o00")
        },
        Case {
            attrs: &[("BulletsTextAfter", " ")],
            ..bullet("local after space (= c02)", "o01")
        },
        Case {
            attrs: &[("NumberingExpression", "^#.")],
            ..number("local expr ^#. (= c09)", "o02")
        },
        Case {
            attrs: &[("NumberingExpression", "(^#)^t")],
            left_indent: 24.0,
            first_line_indent: -24.0,
            ..number("local expr (^#)^t · hanging 24", "o03")
        },
        Case {
            style: Some(STYLE_AFTER_SPACE),
            attrs: &[("BulletsTextAfter", "^t")],
            ..bullet("style space, local ^t", "o04")
        },
        Case {
            style: Some(STYLE_AFTER_TAB),
            attrs: &[("BulletsTextAfter", " ")],
            ..bullet("style ^t, local space", "o05")
        },
        Case {
            style: Some(STYLE_NUMBER_NO_TAB),
            attrs: &[("NumberingExpression", "^#.^t")],
            ..number("style ^#., local ^#.^t", "o06")
        },
        Case {
            props: &[("NumberingExpression", "string", "(^#)")],
            ..number("expr (^#) as <Properties>", "o07")
        },
        Case {
            props: &[("BulletsTextAfter", "string", ">")],
            ..bullet("after > as <Properties>", "o08")
        },
        Case {
            first: &[("NumberingContinue", "false"), ("NumberingStartAt", "5")],
            ..number("first: continue false, start 5", "o09")
        },
        Case {
            first: &[("NumberingStartAt", "5")],
            ..number("first: start 5 only", "o10")
        },
        Case {
            second: &[("NumberingContinue", "false")],
            ..number("second: continue false", "o11")
        },
        Case {
            props: &[("NumberingFormat", "string", "I, II, III, IV...")],
            ..number("format I, II as <Properties>", "o12")
        },
        Case {
            attrs: &[("NumberingFormat", "I, II, III, IV...")],
            ..number("format I, II as attribute", "o13")
        },
        Case {
            attrs: &[("BulletsTextAfter", " ")],
            props: &[("BulletsCharacterStyle", "object", CHAR_BIG)],
            ..bullet("bullet char style <Properties>", "o14")
        },
        Case {
            attrs: &[
                ("BulletsTextAfter", " "),
                ("BulletsCharacterStyle", CHAR_BIG),
            ],
            ..bullet("bullet char style attribute", "o15")
        },
        Case {
            attrs: &[("NumberingExpression", "^#.")],
            props: &[("NumberingCharacterStyle", "object", CHAR_BIG)],
            ..number("number char style <Properties>", "o16")
        },
        Case {
            attrs: &[
                ("NumberingExpression", "^#."),
                ("BulletsAndNumberingDigitsCharacterStyle", CHAR_BIG),
            ],
            ..number("digits char style attribute", "o17")
        },
        Case {
            first: &[("NumberingContinue", "true"), ("NumberingStartAt", "5")],
            ..number("first: continue true, start 5", "o18")
        },
        Case {
            style: Some(STYLE_START1_CONTINUE),
            ..number("style start 1 + continue true", "o19")
        },
        Case {
            style: Some(STYLE_START5),
            ..number("style start 5 only", "o20")
        },
        Case {
            second: &[("NumberingStartAt", "10")],
            ..number("second: start 10 only", "o21")
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
    let style = |self_id: &str, name: &str, attr: &str| {
        format!(
            "<ParagraphStyle Self=\"{self_id}\" Name=\"{name}\" {attr}>\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn></Properties>\
</ParagraphStyle>"
        )
    };
    format!(
        "<RootCharacterStyleGroup><CharacterStyle Self=\"{CHAR_BIG}\" Name=\"Big\" \
PointSize=\"20\"/></RootCharacterStyleGroup>\
<RootParagraphStyleGroup>{}{}{}{}{}</RootParagraphStyleGroup>",
        style(STYLE_AFTER_TAB, "AfterTab", "BulletsTextAfter=\"^t\""),
        style(STYLE_AFTER_SPACE, "AfterSpace", "BulletsTextAfter=\" \""),
        style(
            STYLE_NUMBER_NO_TAB,
            "NumberNoTab",
            "NumberingExpression=\"^#.\""
        ),
        style(
            STYLE_START1_CONTINUE,
            "Start1Continue",
            "NumberingStartAt=\"1\" NumberingContinue=\"true\""
        ),
        style(STYLE_START5, "Start5", "NumberingStartAt=\"5\""),
    )
}

fn paragraph(c: &Case, word: &str, only: Attrs) -> Paragraph {
    let mut attrs = vec![("Hyphenation", "false")];
    if let Some(s) = c.style {
        attrs.push(("AppliedParagraphStyle", s));
    }
    attrs.extend_from_slice(c.attrs);
    attrs.extend_from_slice(only);
    Paragraph {
        extra_paragraph_attrs: attrs,
        extra_paragraph_props: c.props.to_vec(),
        leading: Some(LEADING),
        first_line_indent: Some(c.first_line_indent),
        left_indent: Some(c.left_indent),
        bullets_list_type: Some(c.list),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: format!("{} {word}", c.tag),
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
                paragraphs: vec![
                    paragraph(case, "one", case.first),
                    paragraph(case, "two", case.second),
                ],
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
