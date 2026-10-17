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

//! `endnotes.idml` — endnotes as InDesign writes them (core ADR
//! 170), asked of InDesign 20.0.1 (`endnote-probe`).
//!
//! Two pages. Page 1's story carries three endnote references, each an
//! `<Endnote Self EndnoteTextRange/>` on a superscript range of its own;
//! page 2 holds the endnote story (`IsEndnoteStory="true"`) in an
//! `<EndnoteTextFrame>`: the title,
//! then per note an `<EndnoteRange SourceEndnote>` whose text opens with
//! `\u{FEFF}<?ACE 4?>` (the note's number) and the separator, and closes
//! with `\u{FEFF}`. `<EndnoteOption>` stands in the design map, its
//! numbering starting at 3 in lower roman so the numbers are visibly the
//! options'.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw},
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

pub const SAMPLE: &str = "endnotes";
const PAGE_W_PT: f32 = 400.0;
const PAGE_H_PT: f32 = 300.0;
const FONT: &str = "Inter";

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");
const NO_CHAR: &str = "CharacterStyle/$ID/[No character style]";
const BODY_STYLE: &str = "ParagraphStyle/Body";

/// The body: text before each reference, the note's text.
pub const NOTES: [(&str, &str); 3] = [
    ("The first claim needs a source.", "The first endnote."),
    (
        " A second claim needs another.",
        "The second endnote, a little longer.",
    ),
    (" And a third.", "The third endnote."),
];
pub const TITLE: &str = "Endnotes";

pub fn story_id(name: &str) -> String {
    self_id(SAMPLE, &format!("Story{name}"), 0)
}

pub fn endnote_id(i: usize) -> String {
    format!("uen{}", i + 1)
}

fn range_id(i: usize) -> String {
    format!("uer{}", i + 1)
}

fn csr(b: &mut XmlBuilder, extra: &[(&str, &str)]) {
    let mut attrs = vec![("AppliedCharacterStyle", NO_CHAR)];
    attrs.extend_from_slice(extra);
    b.start("CharacterStyleRange", &attrs);
}

fn content(b: &mut XmlBuilder, text: &str) {
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
}

fn body_story() -> Vec<u8> {
    let id = story_id("Body");
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start(
        "Story",
        &[("Self", id.as_str()), ("IsEndnoteStory", "false")],
    );
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", BODY_STYLE)],
    );
    for (i, (before, _)) in NOTES.iter().enumerate() {
        csr(&mut b, &[]);
        content(&mut b, before);
        b.end("CharacterStyleRange");
        csr(&mut b, &[("Position", "Superscript")]);
        let (e, r) = (endnote_id(i), range_id(i));
        b.empty(
            "Endnote",
            &[("Self", e.as_str()), ("EndnoteTextRange", r.as_str())],
        );
        b.end("CharacterStyleRange");
    }
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

fn endnote_story() -> Vec<u8> {
    let id = story_id("Endnotes");
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start(
        "Story",
        &[("Self", id.as_str()), ("IsEndnoteStory", "true")],
    );
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", BODY_STYLE)],
    );
    csr(&mut b, &[]);
    content(&mut b, TITLE);
    for (i, (_, note)) in NOTES.iter().enumerate() {
        b.empty("Br", &[]);
        let (e, r) = (endnote_id(i), range_id(i));
        b.start(
            "EndnoteRange",
            &[("Self", r.as_str()), ("SourceEndnote", e.as_str())],
        );
        b.start("Content", &[]);
        b.text("\u{FEFF}");
        b.write_pi("ACE", "4");
        b.text(&format!("\t{note}\u{FEFF}"));
        b.end("Content");
        b.end("EndnoteRange");
    }
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// `<EndnoteOption>` as InDesign writes it, numbering from 3 in lower
/// roman.
fn endnote_option() -> String {
    format!(
        "<EndnoteOption EndnoteTitle=\"{TITLE}\" EndnoteTitleStyle=\"{BODY_STYLE}\" \
StartEndnoteNumberAt=\"3\" EndnoteMarkerStyle=\"{NO_CHAR}\" EndnoteTextStyle=\"{BODY_STYLE}\" \
EndnoteSeparatorText=\"&#x9;\" EndnotePrefix=\"\" EndnoteSuffix=\"\"><Properties>\
<EndnoteNumberingStyle type=\"enumeration\">LowerRoman</EndnoteNumberingStyle>\
<RestartEndnoteNumbering type=\"enumeration\">Continuous</RestartEndnoteNumbering>\
<EndnoteMarkerPositioning type=\"enumeration\">SuperscriptMarker</EndnoteMarkerPositioning>\
<ScopeValue type=\"enumeration\">EndnoteDocumentScope</ScopeValue>\
<FrameCreateOption type=\"enumeration\">NewPage</FrameCreateOption>\
<ShowEndnotePrefixSuffix type=\"enumeration\">NoPrefixSuffix</ShowEndnotePrefixSuffix>\
</Properties></EndnoteOption>"
    )
}

fn styles() -> Vec<u8> {
    styles_xml_with_raw(&format!(
        "<RootParagraphStyleGroup>\
<ParagraphStyle Self=\"{BODY_STYLE}\" Name=\"Body\" FontStyle=\"Regular\" PointSize=\"10\" \
SpaceAfter=\"4\" Hyphenation=\"false\" FillColor=\"Color/Black\">\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn>\
<Leading type=\"unit\">12</Leading>\
<AppliedFont type=\"string\">{FONT}</AppliedFont></Properties></ParagraphStyle>\
</RootParagraphStyleGroup>"
    ))
}

fn frame(id: String, story: String) -> PageItem {
    Rect {
        self_id: id,
        width_pt: 320.0,
        height_pt: 240.0,
        item_transform: translate(40.0, 30.0),
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
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

pub fn build() -> Sample {
    let s = SAMPLE;
    let pages = vec![
        vec![frame(self_id(s, "FrameBody", 0), story_id("Body"))],
        vec![frame(self_id(s, "FrameEndnotes", 0), story_id("Endnotes"))],
    ];
    let master_id = self_id(s, "MasterSpread", 0);
    let spreads: Vec<(String, Vec<u8>)> = pages
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
            // The endnote story's frame is an `<EndnoteTextFrame>`, as
            // InDesign writes it: in a `<TextFrame>` InDesign does not take
            // the story for the document's endnotes.
            let xml = if i == 1 {
                String::from_utf8(xml)
                    .expect("utf-8")
                    .replace("<TextFrame ", "<EndnoteTextFrame ")
                    .replace("</TextFrame>", "</EndnoteTextFrame>")
                    .into_bytes()
            } else {
                xml
            };
            (id, xml)
        })
        .collect();
    let stories = vec![
        (story_id("Body"), body_story()),
        (story_id("Endnotes"), endnote_story()),
    ];
    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
        stories: stories.iter().map(|(id, _)| id.clone()).collect(),
    });
    let mut designmap = String::from_utf8(designmap).expect("utf-8");
    let at = designmap.find("<idPkg:Spread").expect("spread include");
    designmap.insert_str(at, &endnote_option());
    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap.into_bytes(),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
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
