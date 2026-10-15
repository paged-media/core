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

//! `column-rules.idml` — the rules InDesign draws between a text frame's
//! columns, asked of InDesign (A-19).
//!
//! One Letter page, a 3 × 3 grid of 168 × 180 pt text frames, each case
//! labelled above its frame. Open Sans 8 pt on 10 pt, hyphenation off,
//! enough text to fill every column. Unless a case says otherwise: two
//! columns, a 12 pt gutter, a 1 pt solid black rule at 100 %, no insets,
//! no offset, the frame's own inset 0.
//!
//! 1. the default rule;
//! 2. three columns, a 9 pt gutter, 0.5 pt at 50 % tint;
//! 3. a 2 pt `Dashed` rule;
//! 4. top inset 12 and bottom inset 24, unchained;
//! 5. a horizontal offset of 4 pt;
//! 6. insets of −8 pt (the rule overshoots the frame);
//! 7. the frame inset 10 pt on every side;
//! 8. three lines of text only (the second column empty);
//! 9. one column, rules switched on.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{ColumnRulePref, PageItem, Rect, TextFramePref},
    resources::{
        container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml, BUILTIN_STROKE_STYLES,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "column-rules";
const PAGE_W_PT: f32 = 612.0;
const PAGE_H_PT: f32 = 792.0;
const FONT: &str = "Open Sans";
const SIZE: f32 = 8.0;
const LEADING: f32 = 10.0;
const LABEL_SIZE: f32 = 6.0;
pub const COLUMNS: [f32; 3] = [36.0, 222.0, 408.0];
pub const ROWS: [f32; 3] = [72.0, 300.0, 528.0];
pub const FRAME_W: f32 = 168.0;
pub const FRAME_H: f32 = 180.0;
const GUTTER: f32 = 12.0;

const TEXT: &str = "Column rules sit in the gutter between the columns of a text frame. \
    They run from the top of the frame to its bottom unless an inset says otherwise, and \
    they take a weight, a stroke style, a colour and a tint like any other stroke. A frame \
    with two columns draws one rule, a frame with three columns draws two, and a frame with \
    a single column draws none at all. The text in each column is set as usual and wraps \
    from the first column to the next one when it reaches the bottom of the frame. This \
    paragraph is long enough to fill every column of every frame on the page, so each rule \
    stands between two columns of type. ";

pub const RULE: ColumnRulePref = ColumnRulePref {
    stroke_width: 1.0,
    stroke_type: "StrokeStyle/$ID/Solid",
    stroke_color: "Color/Black",
    stroke_tint: 100.0,
    top_inset: 0.0,
    bottom_inset: 0.0,
    inset_chain: true,
    offset: 0.0,
};

/// One case: its label, its text, its columns and its rule.
pub struct Case {
    pub label: &'static str,
    pub columns: u32,
    pub gutter: f32,
    pub inset: f32,
    pub rule: ColumnRulePref,
    pub short: bool,
}

fn case(label: &'static str, rule: ColumnRulePref) -> Case {
    Case {
        label,
        columns: 2,
        gutter: GUTTER,
        inset: 0.0,
        rule,
        short: false,
    }
}

/// The cases, row by row.
pub fn cases() -> Vec<Case> {
    vec![
        case("1 default rule", RULE),
        Case {
            columns: 3,
            gutter: 9.0,
            ..case(
                "2 three columns, 0.5 pt, tint 50",
                ColumnRulePref {
                    stroke_width: 0.5,
                    stroke_tint: 50.0,
                    ..RULE
                },
            )
        },
        case(
            "3 Dashed 2 pt",
            ColumnRulePref {
                stroke_width: 2.0,
                stroke_type: "StrokeStyle/$ID/Dashed",
                ..RULE
            },
        ),
        case(
            "4 insets 12 / 24",
            ColumnRulePref {
                top_inset: 12.0,
                bottom_inset: 24.0,
                inset_chain: false,
                ..RULE
            },
        ),
        case(
            "5 offset 4",
            ColumnRulePref {
                offset: 4.0,
                ..RULE
            },
        ),
        case(
            "6 insets -8",
            ColumnRulePref {
                top_inset: -8.0,
                bottom_inset: -8.0,
                ..RULE
            },
        ),
        Case {
            inset: 10.0,
            ..case("7 frame inset 10", RULE)
        },
        Case {
            short: true,
            ..case("8 three lines only", RULE)
        },
        Case {
            columns: 1,
            ..case("9 one column", RULE)
        },
    ]
}

fn run(text: &str, size: f32) -> Run {
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
        anchored_frame: None,
    }
}

fn paragraph(text: &str, size: f32, leading: f32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(leading),
        justification: Some("LeftAlign"),
        runs: vec![run(text, size)],
        ..Paragraph::plain("")
    }
}

fn frame(
    id: String,
    story: String,
    size: (f32, f32),
    at: (f32, f32),
    pref: TextFramePref,
) -> PageItem {
    Rect {
        self_id: id,
        width_pt: size.0,
        height_pt: size.1,
        item_transform: translate(at.0, at.1),
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
        text_frame_pref: Some(pref),
        custom_subpaths: None,
    }
    .into()
}

fn story(stories: &mut Vec<(String, Vec<u8>)>, id: String, paragraph: Paragraph) -> String {
    stories.push((
        id.clone(),
        write_story(&Story {
            extra_story_attrs: Vec::new(),
            self_id: id.clone(),
            paragraphs: vec![paragraph],
        }),
    ));
    id
}

/// Graphic.xml declaring InDesign's built-in stroke styles: InDesign
/// resolves a built-in `StrokeType` only against its declaration in the
/// package (see `stroke_styles`).
fn graphic_xml_with_builtins() -> Vec<u8> {
    let decls: String = BUILTIN_STROKE_STYLES
        .iter()
        .map(|name| format!(r#"<StrokeStyle Self="StrokeStyle/$ID/{name}" Name="$ID/{name}"/>"#))
        .collect();
    let xml = String::from_utf8(graphic_xml()).expect("utf-8");
    xml.replacen("</idPkg:Graphic>", &format!("{decls}</idPkg:Graphic>"), 1)
        .into_bytes()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut items = Vec::new();
    for (n, c) in cases().into_iter().enumerate() {
        let n = n as u32;
        let (x, y) = (COLUMNS[n as usize % 3], ROWS[n as usize / 3]);
        let l = story(
            &mut stories,
            self_id(SAMPLE, "LabelStory", n),
            paragraph(c.label, LABEL_SIZE, LABEL_SIZE * 1.2),
        );
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", n),
            l,
            (FRAME_W, 9.0),
            (x, y - 14.0),
            TextFramePref {
                inset_spacing: Some([0.0; 4]),
                ..Default::default()
            },
        ));
        let text = if c.short {
            "Three lines of text, and the second column stays empty below the rule."
        } else {
            &TEXT.repeat(2)
        };
        let b = story(
            &mut stories,
            self_id(SAMPLE, "Story", n),
            paragraph(text, SIZE, LEADING),
        );
        items.push(frame(
            self_id(SAMPLE, "Frame", n),
            b,
            (FRAME_W, FRAME_H),
            (x, y),
            TextFramePref {
                inset_spacing: Some([c.inset; 4]),
                text_column_count: Some(c.columns),
                text_column_gutter: Some(c.gutter),
                column_rule: Some(c.rule),
                ..Default::default()
            },
        ));
    }
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml_with_builtins(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", 0),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        )],
        spreads: vec![(
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id,
                page_self_id: self_id(SAMPLE, "Page", 0),
                page_name: "1".to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        )],
        stories,
    }
}
