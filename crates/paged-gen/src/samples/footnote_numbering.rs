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

//! `footnote-numbering.idml` and `footnote-restart.idml` — how InDesign
//! numbers the footnotes of a document, asked of InDesign (L-01c, L-01g).
//!
//! The same two pages in both, one document-wide `<FootnoteOption>` each:
//!
//! * page 1: story A's first frame on top, story B below it;
//! * page 2: story A's second frame on top (A threads on to it, its third
//!   paragraph starting the next frame), story C below it.
//!
//! Every paragraph carries one note. The designmap lists the stories C,
//! B, A — the reverse of where they stand — so the export shows whether
//! notes are numbered in page order or story order.
//!
//! * `footnote-numbering`: `RestartNumbering` `DontRestart`, `StartAt`
//!   1, `Prefix` "[" and `Suffix` "]" shown in both the reference and the
//!   note (`PrefixSuffixBoth`).
//! * `footnote-restart`: `PageRestart`, `LowerRoman`, `Prefix` "(" and
//!   `Suffix` ")" in the reference only (`PrefixSuffixReference`).

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
use crate::xml::XmlBuilder;

const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
pub const FRAME_W: f32 = 420.0;
pub const FRAME_H: f32 = 260.0;
pub const TOP: (f32, f32) = (88.0, 90.0);
pub const BOTTOM: (f32, f32) = (88.0, 450.0);

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

/// One document-wide footnote option set.
pub struct Variant {
    pub sample: &'static str,
    pub start_at: u32,
    pub prefix: &'static str,
    pub suffix: &'static str,
    pub numbering_style: &'static str,
    pub restart: &'static str,
    pub show_prefix_suffix: &'static str,
}

pub const NUMBERING: Variant = Variant {
    sample: "footnote-numbering",
    start_at: 1,
    prefix: "[",
    suffix: "]",
    numbering_style: "Arabic",
    restart: "DontRestart",
    show_prefix_suffix: "PrefixSuffixBoth",
};

pub const RESTART: Variant = Variant {
    sample: "footnote-restart",
    start_at: 1,
    prefix: "(",
    suffix: ")",
    numbering_style: "LowerRoman",
    restart: "PageRestart",
    show_prefix_suffix: "PrefixSuffixReference",
};

/// The stories: `(name, paragraphs)`. A paragraph whose text starts with
/// `>` starts the next frame.
pub fn stories() -> [(&'static str, Vec<&'static str>); 3] {
    [
        (
            "A",
            vec![
                "A1 The first paragraph of story A, on page one.",
                "A2 The second paragraph of story A, still on page one.",
                ">A3 The third paragraph of story A starts its second frame, on page two.",
                "A4 The fourth paragraph of story A, on page two.",
            ],
        ),
        (
            "B",
            vec![
                "B1 The first paragraph of story B, below story A on page one.",
                "B2 The second paragraph of story B.",
            ],
        ),
        (
            "C",
            vec![
                "C1 The first paragraph of story C, below story A on page two.",
                "C2 The second paragraph of story C.",
            ],
        ),
    ]
}

pub fn story_id(sample: &str, name: &str) -> String {
    self_id(sample, &format!("Story{name}"), 0)
}

fn start_range(b: &mut XmlBuilder, extra: &[(&str, &str)], size: &str) {
    let mut attrs = vec![
        (
            "AppliedCharacterStyle",
            "CharacterStyle/$ID/[No character style]",
        ),
        ("FontStyle", "Regular"),
        ("PointSize", size),
        ("FillColor", "Color/Black"),
    ];
    attrs.extend_from_slice(extra);
    b.start("CharacterStyleRange", &attrs);
    b.start("Properties", &[]);
    b.start("AppliedFont", &[("type", "string")]);
    b.text(BODY_FONT);
    b.end("AppliedFont");
    b.end("Properties");
}

fn content(b: &mut XmlBuilder, text: &str) {
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
}

/// A `<Footnote>` as InDesign writes one: its number (`<?ACE 4?>`) and the
/// separator open the note's text.
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

fn story_xml(sample: &str, name: &str, paragraphs: &[&str]) -> Vec<u8> {
    let id = story_id(sample, name);
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    let last = paragraphs.len() - 1;
    for (pi, raw) in paragraphs.iter().enumerate() {
        let (next_frame, text) = match raw.strip_prefix('>') {
            Some(t) => (true, t),
            None => (false, *raw),
        };
        let mut attrs = vec![
            (
                "AppliedParagraphStyle",
                "ParagraphStyle/$ID/[No paragraph style]",
            ),
            ("Hyphenation", "false"),
            ("SpaceAfter", "6"),
        ];
        if next_frame {
            attrs.push(("StartParagraph", "NextFrame"));
        }
        b.start("ParagraphStyleRange", &attrs);
        start_range(&mut b, &[], "10");
        content(&mut b, text);
        b.end("CharacterStyleRange");
        start_range(&mut b, &[("Position", "Superscript")], "10");
        let note = format!("The note on {}.", &text[..2]);
        footnote(
            &mut b,
            self_id(sample, &format!("Footnote{name}"), pi as u32),
            &note,
        );
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

/// `Resources/Preferences.xml` with the variant's `<FootnoteOption>`,
/// its enumerations as typed `<Properties>` children (InDesign's own
/// spelling).
fn preferences_xml(v: &Variant) -> Vec<u8> {
    let start_at = v.start_at.to_string();
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Preferences", &[PKG_NS, DOM_VERSION]);
    b.empty("TextPreference", &[("UseOpticalSize", "false")]);
    b.start(
        "FootnoteOption",
        &[
            ("StartAt", start_at.as_str()),
            ("Prefix", v.prefix),
            ("Suffix", v.suffix),
            ("SeparatorText", "\t"),
            ("SpaceBetween", "0"),
            ("Spacer", "0"),
            ("RuleOn", "true"),
            ("RuleLineWeight", "1"),
            ("RuleWidth", "72"),
        ],
    );
    b.start("Properties", &[]);
    for (name, value) in [
        ("FootnoteNumberingStyle", v.numbering_style),
        ("RestartNumbering", v.restart),
        ("ShowPrefixSuffix", v.show_prefix_suffix),
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
    at: (f32, f32),
    prev: Option<String>,
    next: Option<String>,
) -> PageItem {
    Rect {
        self_id: id,
        width_pt: FRAME_W,
        height_pt: FRAME_H,
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
    let stories: Vec<(String, Vec<u8>)> = stories()
        .iter()
        .map(|(name, paragraphs)| (story_id(s, name), story_xml(s, name, paragraphs)))
        .collect();
    let a1 = self_id(s, "FrameA", 1);
    let a2 = self_id(s, "FrameA", 2);
    let page_one = vec![
        frame(a1.clone(), story_id(s, "A"), TOP, None, Some(a2.clone())),
        frame(
            self_id(s, "FrameB", 1),
            story_id(s, "B"),
            BOTTOM,
            None,
            None,
        ),
    ];
    let page_two = vec![
        frame(a2, story_id(s, "A"), TOP, Some(a1), None),
        frame(
            self_id(s, "FrameC", 1),
            story_id(s, "C"),
            BOTTOM,
            None,
            None,
        ),
    ];
    let master_id = self_id(s, "MasterSpread", 0);
    let spreads: Vec<(String, Vec<u8>)> = [page_one, page_two]
        .into_iter()
        .enumerate()
        .map(|(i, items)| {
            let id = self_id(s, "Spread", i as u32);
            let xml = write_spread(&Spread {
                self_id: id.clone(),
                page_self_id: self_id(s, "Page", i as u32),
                page_name: (i + 1).to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            });
            (id, xml)
        })
        .collect();
    // The designmap lists the stories C, B, A: the reverse of the page.
    let mut story_order: Vec<String> = stories.iter().map(|(id, _)| id.clone()).collect();
    story_order.reverse();
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: story_order,
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
        spreads,
        stories,
    }
}
