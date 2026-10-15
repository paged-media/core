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

//! `footnote-columns.idml` and `footnote-straddle.idml` — where InDesign
//! sets the footnotes of a frame with columns, asked of InDesign (L-01d).
//!
//! One A4 page, two frames of two columns each:
//!
//! * frame A on top, story A: six paragraphs, the fourth starting the
//!   second column (`StartParagraph="NextColumn"`);
//! * frame B below it, story B: four paragraphs, the third starting the
//!   second column. Frame B overrides the document's footnote layout
//!   (`FootnotesEnableOverrides`), spanning its columns when the document
//!   does not and the other way round.
//!
//! Every paragraph carries one note, long enough to wrap differently at a
//! column's width and at the frame's.
//!
//! * `footnote-columns`: the document does not span (`EnableStraddling`
//!   false); frame B spans.
//! * `footnote-straddle`: the document spans; frame B does not.

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
use crate::samples::footnote_numbering::{content, footnote, start_range};
use crate::xml::XmlBuilder;

const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
pub const FRAME_W: f32 = 420.0;
pub const FRAME_H: f32 = 300.0;
pub const GUTTER: f32 = 12.0;
pub const TOP: (f32, f32) = (88.0, 70.0);
pub const BOTTOM: (f32, f32) = (88.0, 440.0);

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

/// Whether the document spans its footnotes across columns; frame B says
/// the opposite.
pub struct Variant {
    pub sample: &'static str,
    pub straddle: bool,
}

pub const COLUMNS: Variant = Variant {
    sample: "footnote-columns",
    straddle: false,
};

pub const STRADDLE: Variant = Variant {
    sample: "footnote-straddle",
    straddle: true,
};

/// The stories: `(name, paragraphs)`. A paragraph whose text starts with
/// `>` starts the next column.
pub fn stories() -> [(&'static str, Vec<&'static str>); 2] {
    [
        (
            "A",
            vec![
                "A1 The first paragraph of story A, in the first column.",
                "A2 The second paragraph of story A.",
                "A3 The third paragraph of story A.",
                ">A4 The fourth paragraph starts the second column.",
                "A5 The fifth paragraph of story A.",
                "A6 The sixth paragraph of story A.",
            ],
        ),
        (
            "B",
            vec![
                "B1 The first paragraph of story B, in the first column.",
                "B2 The second paragraph of story B.",
                ">B3 The third paragraph starts the second column.",
                "B4 The fourth paragraph of story B.",
            ],
        ),
    ]
}

pub fn story_id(sample: &str, name: &str) -> String {
    self_id(sample, &format!("Story{name}"), 0)
}

fn story_xml(sample: &str, name: &str, paragraphs: &[&str]) -> Vec<u8> {
    let id = story_id(sample, name);
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    let last = paragraphs.len() - 1;
    for (pi, raw) in paragraphs.iter().enumerate() {
        let (next_column, text) = match raw.strip_prefix('>') {
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
        if next_column {
            attrs.push(("StartParagraph", "NextColumn"));
        }
        b.start("ParagraphStyleRange", &attrs);
        start_range(&mut b, &[], "10");
        content(&mut b, text);
        b.end("CharacterStyleRange");
        start_range(&mut b, &[("Position", "Superscript")], "10");
        let note = format!(
            "The note on {}, long enough to wrap at a column's width.",
            &text[..2]
        );
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

/// `Resources/Preferences.xml` with the variant's `<FootnoteOption>`.
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
            (
                "EnableStraddling",
                if v.straddle { "true" } else { "false" },
            ),
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

fn frame(id: String, story: String, at: (f32, f32), span: Option<bool>) -> PageItem {
    Rect {
        self_id: id,
        width_pt: FRAME_W,
        height_pt: FRAME_H,
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
        text_frame_pref: Some(TextFramePref {
            inset_spacing: Some([0.0; 4]),
            first_baseline_offset: Some("AscentOffset"),
            text_column_count: Some(2),
            text_column_gutter: Some(GUTTER),
            footnotes_span_across_columns: span,
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
    let items = vec![
        frame(self_id(s, "FrameA", 0), story_id(s, "A"), TOP, None),
        frame(
            self_id(s, "FrameB", 0),
            story_id(s, "B"),
            BOTTOM,
            Some(!v.straddle),
        ),
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
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
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
        stories,
    }
}
