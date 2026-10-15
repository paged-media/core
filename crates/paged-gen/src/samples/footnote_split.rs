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

//! `footnote-split.idml` and `footnote-nosplit.idml` — what InDesign does
//! with a note too tall for the room under its reference, asked of
//! InDesign (L-01e).
//!
//! One A4 page, one story threaded through two frames: frame 1 on top,
//! frame 2 below it. The story's first paragraph runs to near frame 1's
//! foot and carries a long note (fifteen lines); the two paragraphs after
//! it carry one-line notes. The document draws a rule above a continued
//! note (`ContinuingRuleOn`, 1 pt, 288 pt wide).
//!
//! * `footnote-split`: `NoSplitting` false — InDesign may split the note
//!   between the two frames.
//! * `footnote-nosplit`: `NoSplitting` true.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, styles_xml},
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::samples::footnote_numbering::{content, start_range};
use crate::xml::XmlBuilder;

const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
pub const FRAME_W: f32 = 420.0;
pub const FRAME_1: (f32, f32, f32) = (88.0, 70.0, 260.0); // x, y, height
pub const FRAME_2: (f32, f32, f32) = (88.0, 440.0, 330.0);

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub struct Variant {
    pub sample: &'static str,
    pub no_splitting: bool,
}

pub const SPLIT: Variant = Variant {
    sample: "footnote-split",
    no_splitting: false,
};

pub const NOSPLIT: Variant = Variant {
    sample: "footnote-nosplit",
    no_splitting: true,
};

/// Numbered sentences, so every line reads differently in the PDF.
fn sentences(tag: &str, n: usize) -> String {
    (1..=n)
        .map(|i| format!("{tag} sentence {i} keeps the line full of ordinary words."))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `(paragraph text, note text)`.
pub fn paragraphs() -> Vec<(String, String)> {
    vec![
        (sentences("P1", 22), sentences("N1", 21)),
        (sentences("P2", 5), "N2 is a short note.".to_string()),
        (sentences("P3", 5), "N3 is a short note.".to_string()),
    ]
}

pub fn story_id(sample: &str) -> String {
    self_id(sample, "Story", 0)
}

fn footnote(b: &mut XmlBuilder, id: String, text: &str) {
    b.start("Footnote", &[("Self", id.as_str())]);
    b.start(
        "ParagraphStyleRange",
        &[(
            "AppliedParagraphStyle",
            "ParagraphStyle/$ID/[No paragraph style]",
        )],
    );
    start_range(b, &[], "8");
    b.start("Content", &[]);
    b.write_pi("ACE", "4");
    b.text(&format!("\t{text}"));
    b.end("Content");
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Footnote");
}

fn story_xml(sample: &str) -> Vec<u8> {
    let id = story_id(sample);
    let paragraphs = paragraphs();
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    let last = paragraphs.len() - 1;
    for (pi, (text, note)) in paragraphs.iter().enumerate() {
        b.start(
            "ParagraphStyleRange",
            &[
                (
                    "AppliedParagraphStyle",
                    "ParagraphStyle/$ID/[No paragraph style]",
                ),
                ("Hyphenation", "false"),
                ("SpaceAfter", "6"),
            ],
        );
        start_range(&mut b, &[], "10");
        content(&mut b, text);
        b.end("CharacterStyleRange");
        start_range(&mut b, &[("Position", "Superscript")], "10");
        footnote(&mut b, self_id(sample, "Footnote", pi as u32), note);
        b.end("CharacterStyleRange");
        if pi < last {
            start_range(&mut b, &[], "10");
            b.empty("Br", &[]);
            b.end("CharacterStyleRange");
        }
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

fn preferences_xml(v: &Variant) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Preferences", &[PKG_NS, DOM_VERSION]);
    b.empty("TextPreference", &[("UseOpticalSize", "false")]);
    b.start(
        "FootnoteOption",
        &[
            ("StartAt", "1"),
            ("SeparatorText", "\t"),
            ("SpaceBetween", "0"),
            ("Spacer", "0"),
            ("RuleOn", "true"),
            ("RuleLineWeight", "1"),
            ("RuleWidth", "72"),
            ("ContinuingRuleOn", "true"),
            ("ContinuingRuleLineWeight", "1"),
            ("ContinuingRuleWidth", "288"),
            ("ContinuingRuleOffset", "0"),
            ("NoSplitting", if v.no_splitting { "true" } else { "false" }),
        ],
    );
    b.start("Properties", &[]);
    for (name, value) in [
        ("FootnoteNumberingStyle", "Arabic"),
        ("RestartNumbering", "DontRestart"),
        ("ShowPrefixSuffix", "NoPrefixSuffix"),
        ("MarkerPositioning", "SuperscriptMarker"),
    ] {
        b.start(name, &[("type", "enumeration")]);
        b.text(value);
        b.end(name);
    }
    b.end("Properties");
    b.end("FootnoteOption");
    b.end("idPkg:Preferences");
    b.into_bytes()
}

fn frame(
    id: String,
    story: String,
    at: (f32, f32, f32),
    prev: Option<String>,
    next: Option<String>,
) -> PageItem {
    Rect {
        self_id: id,
        width_pt: FRAME_W,
        height_pt: at.2,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story),
        next_text_frame: next,
        previous_text_frame: prev,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: Some(TextFramePref {
            inset_spacing: Some([0.0; 4]),
            first_baseline_offset: Some("AscentOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// Build the variant's `Sample`.
pub fn build(v: &Variant) -> Sample {
    let s = v.sample;
    let f1 = self_id(s, "Frame", 1);
    let f2 = self_id(s, "Frame", 2);
    let items = vec![
        frame(f1.clone(), story_id(s), FRAME_1, None, Some(f2.clone())),
        frame(f2, story_id(s), FRAME_2, Some(f1), None),
    ];
    let master_id = self_id(s, "MasterSpread", 0);
    let spread_id = self_id(s, "Spread", 0);
    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: self_id(s, "Page", 0),
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
            stories: vec![story_id(s)],
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(v),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(s, "MasterPage", 0),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        )],
        spreads: vec![(spread_id, spread)],
        stories: vec![(story_id(s), story_xml(s))],
    }
}
