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

//! `notes.idml` — editorial notes as InDesign writes them (core ADR
//! 172), asked of InDesign 20.0.1 (`notes-probe`).
//!
//! One page, one story of two paragraphs holding four `<Note>`s: one inside
//! a word ("not|e"), one between words, one of two paragraphs (`<Br/>` in
//! its range) and one ending the story. Each note stands inline in its
//! `<CharacterStyleRange>` with a range of its own, as InDesign writes it,
//! its author the design map's `<DocumentUser>`. InDesign prints none of
//! them: the page reads as the text without the notes.

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

pub const SAMPLE: &str = "notes";
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
const USER: &str = "dDocumentUser0";
const DATE: &str = "2026-10-16T10:16:46";

/// What stands in the story: text, and notes (their paragraphs).
pub enum Piece {
    Text(&'static str),
    Note(&'static [&'static str]),
}

/// The story's two paragraphs.
pub const PARAGRAPHS: [&[Piece]; 2] = [
    &[
        Piece::Text("Text with a not"),
        Piece::Note(&["Check this claim."]),
        Piece::Text("e here and"),
        Piece::Note(&["A remark between words."]),
        Piece::Text(" more text."),
    ],
    &[
        Piece::Text("A second paragraph"),
        Piece::Note(&["First line of a note.", "Its second paragraph."]),
        Piece::Text(" ends the story."),
        Piece::Note(&["Last one."]),
    ],
];

pub fn story_id() -> String {
    self_id(SAMPLE, "StoryBody", 0)
}

fn content(b: &mut XmlBuilder, text: &str) {
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
}

fn note(b: &mut XmlBuilder, paragraphs: &[&str]) {
    b.start(
        "Note",
        &[
            ("Collapsed", "false"),
            ("CreationDate", DATE),
            ("ModificationDate", DATE),
            ("UserName", "Unbekannter Benutzername"),
            ("AppliedDocumentUser", USER),
        ],
    );
    b.start(
        "ParagraphStyleRange",
        &[(
            "AppliedParagraphStyle",
            "ParagraphStyle/$ID/[No paragraph style]",
        )],
    );
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
    for (i, p) in paragraphs.iter().enumerate() {
        if i > 0 {
            b.empty("Br", &[]);
        }
        content(b, p);
    }
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Note");
}

fn story() -> Vec<u8> {
    let id = story_id();
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", BODY_STYLE)],
    );
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
    for (i, pieces) in PARAGRAPHS.iter().enumerate() {
        if i > 0 {
            b.empty("Br", &[]);
        }
        for piece in pieces.iter() {
            match piece {
                Piece::Text(t) => content(&mut b, t),
                Piece::Note(p) => note(&mut b, p),
            }
        }
    }
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// The `<DocumentUser>` InDesign writes for the notes' author.
fn document_user() -> String {
    format!(
        "<DocumentUser Self=\"{USER}\" UserName=\"$ID/Unknown User Name\"><Properties>\
<UserColor type=\"enumeration\">Gold</UserColor></Properties></DocumentUser>"
    )
}

fn styles() -> Vec<u8> {
    styles_xml_with_raw(&format!(
        "<RootParagraphStyleGroup>\
<ParagraphStyle Self=\"{BODY_STYLE}\" Name=\"Body\" FontStyle=\"Regular\" PointSize=\"12\" \
SpaceAfter=\"6\" Hyphenation=\"false\" FillColor=\"Color/Black\">\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn>\
<Leading type=\"unit\">15</Leading>\
<AppliedFont type=\"string\">{FONT}</AppliedFont></Properties></ParagraphStyle>\
</RootParagraphStyleGroup>"
    ))
}

fn frame() -> PageItem {
    Rect {
        self_id: self_id(SAMPLE, "FrameBody", 0),
        width_pt: 320.0,
        height_pt: 240.0,
        item_transform: translate(40.0, 30.0),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story_id()),
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
    let master_id = self_id(s, "MasterSpread", 0);
    let spread_id = self_id(s, "Spread", 0);
    let spreads = vec![(
        spread_id.clone(),
        write_spread(&Spread {
            self_id: spread_id,
            page_self_id: self_id(s, "Page", 0),
            page_name: "1".to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: vec![frame()],
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        }),
    )];
    let stories = vec![(story_id(), story())];
    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
        stories: stories.iter().map(|(id, _)| id.clone()).collect(),
    });
    let mut designmap = String::from_utf8(designmap).expect("utf-8");
    let at = designmap.find("<idPkg:Spread").expect("spread include");
    designmap.insert_str(at, &document_user());
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
