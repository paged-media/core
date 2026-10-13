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

//! `object-style-options.idml` — the text wrap and anchored-object options
//! an item takes from its object style, asked of InDesign (ADR 120,
//! ADR 143).
//!
//! InDesign leaves the anchored-object options an item takes from its
//! object style off the item, and writes only the ones it overrides: an
//! object of a custom-positioned style that moves itself 20 pt is written
//! `<AnchoredObjectSetting AnchorXoffset="20"/>` (measured on InDesign
//! 20.0.1 through its DOM). Text wrap it writes on every item, but a
//! producer other than InDesign need not. Every object here states no
//! option of its own, unless its case says so:
//!
//!  1. text wrap, bounding box, offsets 4/4/4/12 pt;
//!  2. the same, its category switched off (`EnableTextWrapAndOthers="false"`);
//!  3. the same, with no `EnableTextWrapAndOthers` at all;
//!  4. a style based on case 1's, its category on, setting nothing itself;
//!  5. case 1's style, the object stating `TextWrapMode="None"`;
//!  6. text wrap, jump object;
//!  7. anchored, custom: its top-right corner on the frame's right edge,
//!     6 pt below the anchor's baseline;
//!  8. the same, its category switched off (`EnableAnchoredObjectOptions="false"`);
//!  9. a style based on case 7's, its category on, setting nothing itself;
//! 10. case 7's style, the object stating only `AnchorXoffset="-20"`;
//! 11. anchored above the line, centred;
//! 12. case 7's options with no `EnableAnchoredObjectOptions` at all.
//!
//! The wrap cases (1–6) set a box at the left of a full frame; the
//! anchored cases (7–12) anchor a small box after the frame's first word.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{AnchoredObjectSetting, PageItem, Rect, TextFramePref, TextWrap},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml_with_raw,
        ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "object-style-options";
const PAGE_W_PT: f32 = 595.276; // A4
const PAGE_H_PT: f32 = 841.89;
const FONT: &str = "Open Sans";
const BODY_SIZE: f32 = 8.0;
const LABEL_SIZE: f32 = 7.0;
pub const FRAME_W: f32 = 247.0;
pub const FRAME_H: f32 = 100.0;
const COL_X: [f32; 2] = [36.0, 312.276];
const ROW_Y: [f32; 6] = [80.0, 205.0, 330.0, 455.0, 580.0, 705.0];
/// The wrap cases' box: at the frame's left edge, this far down.
pub const BOX: (f32, f32) = (70.0, 34.0);
pub const BOX_Y: f32 = 20.0;
/// The anchored cases' box.
pub const ANCHORED: (f32, f32) = (24.0, 16.0);
const FRAME_TINT: &str = "Color/FrameTint";
const OBJECT_TINT: &str = "Color/ObjectTint";
/// What case 10's object writes, as InDesign writes an override of one
/// option of its style.
const OWN_X_OFFSET: &str = "<AnchoredObjectSetting AnchorXoffset=\"-20\"/>";

/// What a case's object states of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Own {
    Nothing,
    /// `<TextWrapPreference TextWrapMode="None">`.
    NoWrap,
    /// [`OWN_X_OFFSET`].
    XOffset,
}

/// One case: its label, the style its object applies, what the object
/// states itself, and whether it is anchored in the text.
pub struct Case {
    pub label: &'static str,
    pub style: &'static str,
    pub own: Own,
    pub anchored: bool,
}

const WRAP_TEXT: &str = "Body text set beside a box that states no text wrap of \
its own. Whether these lines step aside for it, and how far, is its object \
style's to say, and only when the style switches the category on. A line \
that meets the box starts after it when the wrap is on and runs straight \
across it when it is off; a jump-object wrap leaves those lines empty and \
carries on below. More words keep the frame full to its foot, so every \
line that meets the box shows the rule at a glance.";
const ANCHOR_LEAD: &str = "Anchored ";
const ANCHOR_TEXT: &str = "here: where the box stands is its object style's \
to say, unless the box says otherwise.";

pub fn cases() -> [Case; 12] {
    let wrap = |label, style, own| Case {
        label,
        style,
        own,
        anchored: false,
    };
    let anchor = |label, style, own| Case {
        label,
        style,
        own,
        anchored: true,
    };
    [
        wrap("1 · style: wrap, bounding box", "OS Wrap", Own::Nothing),
        wrap("2 · style: wrap, category off", "OS Wrap Off", Own::Nothing),
        wrap(
            "3 · style: wrap, no Enable flag",
            "OS Wrap Unflagged",
            Own::Nothing,
        ),
        wrap("4 · based on 1", "OS Wrap Child", Own::Nothing),
        wrap("5 · style 1, object: no wrap", "OS Wrap", Own::NoWrap),
        wrap("6 · style: wrap, jump object", "OS Jump", Own::Nothing),
        anchor(
            "7 · style: anchored, right of the frame",
            "OS Anchor",
            Own::Nothing,
        ),
        anchor(
            "8 · style: anchored, category off",
            "OS Anchor Off",
            Own::Nothing,
        ),
        anchor("9 · based on 7", "OS Anchor Child", Own::Nothing),
        anchor(
            "10 · style 7, object: x offset −20",
            "OS Anchor",
            Own::XOffset,
        ),
        anchor(
            "11 · style: above the line, centred",
            "OS Above",
            Own::Nothing,
        ),
        anchor(
            "12 · style: anchored, no Enable flag",
            "OS Anchor Unflagged",
            Own::Nothing,
        ),
    ]
}

fn object_style(name: &str, enable: &str, based_on: &str, options: &str) -> String {
    format!(
        "<ObjectStyle Self=\"ObjectStyle/{name}\" Name=\"{name}\" {enable}>\
<Properties><BasedOn type=\"object\">{based_on}</BasedOn></Properties>{options}</ObjectStyle>"
    )
}

/// `<TextWrapPreference>` as InDesign writes it on a style.
fn wrap_preference(mode: &str) -> String {
    format!(
        "<TextWrapPreference Inverse=\"false\" ApplyToMasterPageOnly=\"false\" \
TextWrapSide=\"BothSides\" TextWrapMode=\"{mode}\"><Properties>\
<TextWrapOffset Top=\"4\" Left=\"4\" Bottom=\"4\" Right=\"12\"/></Properties>\
</TextWrapPreference>"
    )
}

/// `<AnchoredObjectSetting>` as InDesign writes it on a style: every
/// attribute.
fn anchored_setting(position: &str, point: &str, h_align: &str, y: &str) -> String {
    format!(
        "<AnchoredObjectSetting AnchoredPosition=\"{position}\" SpineRelative=\"false\" \
LockPosition=\"false\" PinPosition=\"true\" AnchorPoint=\"{point}\" \
HorizontalAlignment=\"{h_align}\" HorizontalReferencePoint=\"TextFrame\" \
VerticalAlignment=\"BottomAlign\" VerticalReferencePoint=\"LineBaseline\" \
AnchorXoffset=\"0\" AnchorYoffset=\"{y}\" AnchorSpaceAbove=\"0\"/>"
    )
}

fn styles() -> Vec<u8> {
    const ROOT: &str = "ObjectStyle/$ID/[None]";
    const WRAP_ON: &str = "EnableTextWrapAndOthers=\"true\"";
    const WRAP_OFF: &str = "EnableTextWrapAndOthers=\"false\"";
    const ANCHOR_ON: &str = "EnableAnchoredObjectOptions=\"true\"";
    const ANCHOR_OFF: &str = "EnableAnchoredObjectOptions=\"false\"";
    let bbox = wrap_preference("BoundingBoxTextWrap");
    let custom = anchored_setting("Anchored", "TopRightAnchor", "RightAlign", "6");
    let objects = [
        object_style("OS Wrap", WRAP_ON, ROOT, &bbox),
        object_style("OS Wrap Off", WRAP_OFF, ROOT, &bbox),
        object_style("OS Wrap Unflagged", "", ROOT, &bbox),
        object_style("OS Wrap Child", WRAP_ON, "ObjectStyle/OS Wrap", ""),
        object_style(
            "OS Jump",
            WRAP_ON,
            ROOT,
            &wrap_preference("JumpObjectTextWrap"),
        ),
        object_style("OS Anchor", ANCHOR_ON, ROOT, &custom),
        object_style("OS Anchor Off", ANCHOR_OFF, ROOT, &custom),
        object_style("OS Anchor Child", ANCHOR_ON, "ObjectStyle/OS Anchor", ""),
        object_style(
            "OS Above",
            ANCHOR_ON,
            ROOT,
            &anchored_setting("AboveLine", "BottomLeftAnchor", "CenterAlign", "0"),
        ),
        object_style("OS Anchor Unflagged", "", ROOT, &custom),
    ]
    .concat();
    styles_xml_with_raw(&format!(
        "<RootObjectStyleGroup>{objects}</RootObjectStyleGroup>"
    ))
}

fn run(text: &str, size: f32, anchored_frame: Option<Rect>) -> Run {
    Run {
        extra_char_attrs: Vec::new(),
        text: text.to_string(),
        point_size: Some(size),
        fill_color: Some("Color/Black".to_string()),
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: Some(FONT),
        anchored_frame,
    }
}

fn paragraph(runs: Vec<Run>, size: f32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(size * 1.2),
        justification: Some("LeftAlign"),
        runs,
        ..Paragraph::plain("")
    }
}

fn rect(id: String, size: (f32, f32), at: (f32, f32), fill: Option<&str>) -> Rect {
    Rect {
        self_id: id,
        width_pt: size.0,
        height_pt: size.1,
        item_transform: translate(at.0, at.1),
        fill_color: fill.map(String::from),
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

/// A case's object: named so the probe finds it, applying its style.
fn object(seq: u32, case: &Case, at: (f32, f32)) -> Rect {
    let size = if case.anchored { ANCHORED } else { BOX };
    let mut r = rect(self_id(SAMPLE, "Object", seq), size, at, Some(OBJECT_TINT));
    r.extra_attrs = vec![
        ("Name".to_string(), format!("case{} object", seq + 1)),
        (
            "AppliedObjectStyle".to_string(),
            format!("ObjectStyle/{}", case.style),
        ),
    ];
    match case.own {
        Own::NoWrap => {
            r.text_wrap = Some(TextWrap {
                mode: "None",
                offsets: [0.0; 4],
                side: Some("BothSides"),
            })
        }
        // A marker the story's bytes swap for [`OWN_X_OFFSET`]: the builder
        // writes every attribute of an `<AnchoredObjectSetting>`.
        Own::XOffset => r.anchored_setting = Some(AnchoredObjectSetting::inline()),
        Own::Nothing => {}
    }
    r
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut items: Vec<PageItem> = Vec::new();
    let story = |stories: &mut Vec<(String, Vec<u8>)>, id: String, p: Paragraph| {
        stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs: vec![p],
            }),
        ));
        id
    };
    let text_frame = |id: String,
                      story: String,
                      size: (f32, f32),
                      at: (f32, f32),
                      fill: Option<&str>,
                      name: Option<String>| {
        let mut r = rect(id, size, at, fill);
        r.parent_story = Some(story);
        r.text_frame_pref = Some(TextFramePref {
            inset_spacing: Some([0.0; 4]),
            ..Default::default()
        });
        if let Some(name) = name {
            r.extra_attrs = vec![("Name".to_string(), name)];
        }
        PageItem::from(r)
    };
    let title = story(
        &mut stories,
        self_id(SAMPLE, "TitleStory", 0),
        paragraph(
            vec![run(
                "object-style-options · text wrap and anchored-object options from the object style",
                LABEL_SIZE,
                None,
            )],
            LABEL_SIZE,
        ),
    );
    items.push(text_frame(
        self_id(SAMPLE, "TitleFrame", 0),
        title,
        (520.0, 12.0),
        (36.0, 40.0),
        None,
        None,
    ));
    for (i, c) in cases().iter().enumerate() {
        let seq = i as u32;
        let (x, y) = (COL_X[i % 2], ROW_Y[i / 2]);
        let label = story(
            &mut stories,
            self_id(SAMPLE, "LabelStory", seq),
            paragraph(vec![run(c.label, LABEL_SIZE, None)], LABEL_SIZE),
        );
        items.push(text_frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label,
            (FRAME_W, 10.0),
            (x, y - 13.0),
            None,
            None,
        ));
        let body = if c.anchored {
            let mut obj = object(seq, c, (0.0, 0.0));
            obj.item_transform = IDENTITY;
            paragraph(
                vec![
                    run(ANCHOR_LEAD, BODY_SIZE, None),
                    run(ANCHOR_TEXT, BODY_SIZE, Some(obj)),
                ],
                BODY_SIZE,
            )
        } else {
            items.push(object(seq, c, (x, y + BOX_Y)).into());
            paragraph(vec![run(WRAP_TEXT, BODY_SIZE, None)], BODY_SIZE)
        };
        let story_id = story(&mut stories, self_id(SAMPLE, "Story", seq), body);
        if c.own == Own::XOffset {
            let (_, bytes) = stories.last_mut().expect("the case's story");
            *bytes = own_x_offset(bytes);
        }
        items.push(text_frame(
            self_id(SAMPLE, "Frame", seq),
            story_id,
            (FRAME_W, FRAME_H),
            (x, y),
            Some(FRAME_TINT),
            Some(format!("case{} text", seq + 1)),
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
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml_with_extras(&[
            ExtraColor {
                self_id: FRAME_TINT.to_string(),
                name: "FrameTint".to_string(),
                space: "CMYK",
                value: "0 0 15 0".to_string(),
            },
            ExtraColor {
                self_id: OBJECT_TINT.to_string(),
                name: "ObjectTint".to_string(),
                space: "CMYK",
                value: "45 0 0 0".to_string(),
            },
        ]),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}

/// The story with its object's full `<AnchoredObjectSetting>` replaced by
/// [`OWN_X_OFFSET`].
fn own_x_offset(story: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(story).expect("stories are UTF-8");
    let start = text
        .find("<AnchoredObjectSetting ")
        .expect("case 10's object writes its setting");
    let end = start + text[start..].find("/>").expect("an empty element") + 2;
    format!("{}{OWN_X_OFFSET}{}", &text[..start], &text[end..]).into_bytes()
}
