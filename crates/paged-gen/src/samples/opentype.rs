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

//! `opentype` — OpenType positions and features, on runs and on
//! paragraph styles, asked of InDesign.
//!
//! One frame per case, in faces that carry the features (Inter: `sups`
//! `subs` `sinf` `numr` `dnom` `salt` `case` `frac`; EB Garamond: `sups`
//! `subs` `sinf` `hist` `swsh` (italic) `onum` `lnum` `tnum`; Roboto has
//! no `sups`):
//!
//! - o01–o08: `Position` Superscript beside OTSuperscript, Subscript
//!   beside OTSubscript, OTNumerator / OTDenominator, EB Garamond's
//!   OTSubscript (its `subs` and `sinf` glyphs differ), and
//!   OTSuperscript in a face without `sups`.
//! - o09–o13: `OTFStylisticAlternate`, `Capitalization="AllCaps"` beside
//!   typed capitals (does InDesign switch on `case`?), `OTFHistorical`,
//!   `OTFSwash` on the italic.
//! - o14–o21: the same on a PARAGRAPH STYLE — `Ligatures="false"`,
//!   `OTFFigureStyle`, `OTFStylisticAlternate`, `OTFHistorical`,
//!   `Position="OTSuperscript"`, `OTFFraction` — each beside a control.
//! - o22–o23: `AllCaps` beside typed capitals in Inter with its
//!   contextual alternates off (Inter's `calt` already picks the
//!   case-sensitive punctuation between capitals): does `AllCaps` switch
//!   on `case`?

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

const SAMPLE: &str = "opentype";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
pub const FRAME_W: f32 = 250.0;
const FRAME_H: f32 = 30.0;

pub const INTER: &str = "ParagraphStyle/OtInter";
pub const GARAMOND: &str = "ParagraphStyle/OtGaramond";
pub const GARAMOND_ITALIC: &str = "ParagraphStyle/OtGaramondItalic";
pub const ROBOTO: &str = "ParagraphStyle/OtRoboto";
pub const NO_LIGATURES: &str = "ParagraphStyle/OtNoLigatures";
pub const FIGURES: &str = "ParagraphStyle/OtFigures";
pub const SALT: &str = "ParagraphStyle/OtSalt";
pub const HIST: &str = "ParagraphStyle/OtHist";
pub const SUPERSCRIPT: &str = "ParagraphStyle/OtSuperscript";
pub const FRACTION: &str = "ParagraphStyle/OtFraction";

type Attrs = &'static [(&'static str, &'static str)];

pub struct Case {
    pub label: &'static str,
    pub style: &'static str,
    /// The runs: text and the attributes set on it.
    pub runs: &'static [(&'static str, Attrs)],
}

const SUP: Attrs = &[("Position", "Superscript")];
const OT_SUP: Attrs = &[("Position", "OTSuperscript")];
const SUB: Attrs = &[("Position", "Subscript")];
const OT_SUB: Attrs = &[("Position", "OTSubscript")];
const NONE: Attrs = &[];

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    let c = |label, style, runs| Case { label, style, runs };
    vec![
        c(
            "o01 Inter: Superscript",
            INTER,
            &[("Area 12 m", NONE), ("2", SUP), (" total", NONE)],
        ),
        c(
            "o02 Inter: OTSuperscript",
            INTER,
            &[("Area 12 m", NONE), ("2", OT_SUP), (" total", NONE)],
        ),
        c(
            "o03 Inter: Subscript",
            INTER,
            &[("H", NONE), ("2", SUB), ("O water", NONE)],
        ),
        c(
            "o04 Inter: OTSubscript",
            INTER,
            &[("H", NONE), ("2", OT_SUB), ("O water", NONE)],
        ),
        c(
            "o05 Inter: OTNumerator / OTDenominator",
            INTER,
            &[
                ("1", &[("Position", "OTNumerator")]),
                ("/", NONE),
                ("4", &[("Position", "OTDenominator")]),
                (" cup", NONE),
            ],
        ),
        c(
            "o06 EB Garamond: OTSubscript",
            GARAMOND,
            &[("H", NONE), ("2", OT_SUB), ("O and x", NONE), ("n", OT_SUB)],
        ),
        c(
            "o07 EB Garamond: OTSuperscript",
            GARAMOND,
            &[
                ("M", NONE),
                ("lle", OT_SUP),
                (" and 1", NONE),
                ("st", OT_SUP),
            ],
        ),
        c(
            "o08 Roboto (no sups): OTSuperscript",
            ROBOTO,
            &[("Area 12 m", NONE), ("2", OT_SUP), (" total", NONE)],
        ),
        c(
            "o09 Inter: OTFStylisticAlternate",
            INTER,
            &[
                ("Galaxy ", NONE),
                ("Galaxy Ingot guard", &[("OTFStylisticAlternate", "true")]),
            ],
        ),
        c(
            "o10 Inter: AllCaps",
            INTER,
            &[("(hello)-[world] 10:30", &[("Capitalization", "AllCaps")])],
        ),
        c(
            "o11 Inter: typed capitals",
            INTER,
            &[("(HELLO)-[WORLD] 10:30", NONE)],
        ),
        c(
            "o12 EB Garamond: OTFHistorical",
            GARAMOND,
            &[
                ("Justice is lasting ", NONE),
                ("Justice is lasting", &[("OTFHistorical", "true")]),
            ],
        ),
        c(
            "o13 EB Garamond Italic: OTFSwash",
            GARAMOND_ITALIC,
            &[
                ("Quick Brown ", NONE),
                ("Quick Brown", &[("OTFSwash", "true")]),
            ],
        ),
        c(
            "o14 style Ligatures=false",
            NO_LIGATURES,
            &[("office fluffy affirm", NONE)],
        ),
        c(
            "o15 control: ligatures",
            GARAMOND,
            &[("office fluffy affirm", NONE)],
        ),
        c(
            "o16 style OTFFigureStyle=TabularLining",
            FIGURES,
            &[("Since 1984 to 2026", NONE)],
        ),
        c(
            "o17 control: default figures",
            GARAMOND,
            &[("Since 1984 to 2026", NONE)],
        ),
        c(
            "o18 style OTFStylisticAlternate",
            SALT,
            &[("Galaxy Ingot guard", NONE)],
        ),
        c(
            "o19 style OTFHistorical",
            HIST,
            &[("Justice is lasting", NONE)],
        ),
        c(
            "o20 style Position=OTSuperscript",
            SUPERSCRIPT,
            &[("abc 123 xyz", NONE)],
        ),
        c("o21 style OTFFraction", FRACTION, &[("1/2 and 3/4", NONE)]),
        c(
            "o22 Inter, calt off: AllCaps",
            INTER,
            &[(
                "(hello)-[world] 10:30",
                &[
                    ("Capitalization", "AllCaps"),
                    ("OTFContextualAlternate", "false"),
                ],
            )],
        ),
        c(
            "o23 Inter, calt off: typed capitals",
            INTER,
            &[(
                "(HELLO)-[WORLD] 10:30",
                &[("OTFContextualAlternate", "false")],
            )],
        ),
    ]
}

fn styles_fragment() -> String {
    let para = |id: &str, based_on: &str, attrs: &str, font: Option<&str>| {
        let name = id.trim_start_matches("ParagraphStyle/");
        let based = if based_on.starts_with("$ID/") {
            format!("<BasedOn type=\"string\">{based_on}</BasedOn>")
        } else {
            format!("<BasedOn type=\"object\">{based_on}</BasedOn>")
        };
        let font = font
            .map(|f| format!("<AppliedFont type=\"string\">{f}</AppliedFont>"))
            .unwrap_or_default();
        format!(
            "<ParagraphStyle Self=\"{id}\" Name=\"{name}\" {attrs}><Properties>{based}{font}\
</Properties></ParagraphStyle>"
        )
    };
    let root = "$ID/[No paragraph style]";
    let face = "FillColor=\"Color/Black\" Hyphenation=\"false\"";
    let mut out = String::from("<RootParagraphStyleGroup>");
    out.push_str(&para(
        INTER,
        root,
        &format!("PointSize=\"14\" FontStyle=\"Regular\" {face}"),
        Some("Inter"),
    ));
    out.push_str(&para(
        GARAMOND,
        root,
        &format!("PointSize=\"16\" FontStyle=\"Regular\" {face}"),
        Some("EB Garamond"),
    ));
    out.push_str(&para(
        GARAMOND_ITALIC,
        root,
        &format!("PointSize=\"16\" FontStyle=\"Italic\" {face}"),
        Some("EB Garamond"),
    ));
    out.push_str(&para(
        ROBOTO,
        root,
        &format!("PointSize=\"14\" FontStyle=\"Regular\" {face}"),
        Some("Roboto"),
    ));
    out.push_str(&para(NO_LIGATURES, GARAMOND, "Ligatures=\"false\"", None));
    out.push_str(&para(
        FIGURES,
        GARAMOND,
        "OTFFigureStyle=\"TabularLining\"",
        None,
    ));
    out.push_str(&para(SALT, INTER, "OTFStylisticAlternate=\"true\"", None));
    out.push_str(&para(HIST, GARAMOND, "OTFHistorical=\"true\"", None));
    out.push_str(&para(
        SUPERSCRIPT,
        INTER,
        "Position=\"OTSuperscript\"",
        None,
    ));
    out.push_str(&para(FRACTION, INTER, "OTFFraction=\"true\"", None));
    out.push_str("</RootParagraphStyleGroup>");
    out
}

fn run(text: &str, attrs: Attrs, point_size: Option<f32>) -> Run {
    Run {
        text: text.to_string(),
        point_size,
        fill_color: None,
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: None,
        anchored_frame: None,
        extra_char_attrs: attrs.to_vec(),
    }
}

fn frame(id: String, story: String, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: FRAME_W,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        // No frame strokes: a hairline rasterises differently in the
        // reference (pdftoppm's one-pixel minimum) and would outweigh the
        // text this fixture measures.
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
        text_frame_pref: body.then(|| TextFramePref {
            inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
            first_baseline_offset: Some("LeadingOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// Top-left of case `i`'s body frame, page-local pt.
pub fn frame_origin(i: u32) -> (f32, f32) {
    let col = i % 2;
    let row = i / 2;
    (36.0 + col as f32 * 273.0, 60.0 + row as f32 * 66.0)
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items = Vec::new();

    for (i, c) in cases().iter().enumerate() {
        let seq = i as u32;
        let (x, y) = frame_origin(seq);
        let label_story = self_id(SAMPLE, "LabelStory", seq);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph {
                    runs: vec![run(c.label, NONE, Some(8.0))],
                    ..Paragraph::plain("")
                }],
            }),
        ));
        story_refs.push(label_story.clone());
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label_story,
            14.0,
            (x, y - 16.0),
            false,
        ));

        let body = self_id(SAMPLE, "BodyStory", seq);
        stories.push((
            body.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body.clone(),
                paragraphs: vec![Paragraph {
                    extra_paragraph_attrs: vec![("AppliedParagraphStyle", c.style)],
                    runs: c
                        .runs
                        .iter()
                        .map(|&(text, attrs)| run(text, attrs, None))
                        .collect(),
                    ..Paragraph::plain("")
                }],
            }),
        ));
        story_refs.push(body.clone());
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
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
