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

//! `xref.idml` — cross-references that are out of date, asked of
//! InDesign (core ADR 169).
//!
//! Three pages and one threaded story. Page 1 holds one cross-reference
//! per format to a numbered heading on page 2 and one to a text anchor
//! there, each as InDesign writes a source (`xref-probe`): the format's
//! text in a `<CrossReferenceSource>`, its page number an
//! `XRefPageNumber` text-variable instance. Every source's text is stale:
//! it names "Old Heading" and page 9. The heading's paragraph carries a
//! `<ParagraphDestination>`, the anchor word a `<HyperlinkTextDestination>`,
//! and the design map links each source to its destination.
//!
//! The formats are spelled as an English InDesign spells its defaults.
//! InDesign's PDF of this file is the gate's reference (what it prints
//! before Update Cross-References); what the update writes is recorded by
//! `tools/indesign-export/xref-update-probe.sh` against this file.

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

pub const SAMPLE: &str = "xref";
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
const HEADING_STYLE: &str = "ParagraphStyle/Heading";
/// InDesign's built-in cross-reference page-number variable.
const XREF_PAGE_VARIABLE: &str = "dTextVariablen&lt;?AID 001b?&gt;TV XRefPageNumber";
const XREF_PAGE_NAME: &str = "&lt;?AID 001b?&gt;TV XRefPageNumber";
pub const PARAGRAPH_DESTINATION: &str = "ParagraphDestination/Anchor 1";
pub const TEXT_DESTINATION: &str = "HyperlinkTextDestination/the anchor";

/// A building block: `(BlockType, CustomText)`.
type Block = (&'static str, &'static str);

/// The formats: `(Self, Name, blocks)`, English InDesign's defaults.
pub const FORMATS: [(&str, &str, &[Block]); 5] = [
    (
        "ufmt1",
        "Full Paragraph & Page Number",
        &[
            ("CustomStringBuildingBlock", "\u{201C}"),
            ("FullParagraphBuildingBlock", "$ID/"),
            ("CustomStringBuildingBlock", "\u{201D} on page "),
            ("PageNumberBuildingBlock", "$ID/"),
        ],
    ),
    (
        "ufmt2",
        "Paragraph Text",
        &[
            ("CustomStringBuildingBlock", "\u{201C}"),
            ("ParagraphTextBuildingBlock", "$ID/"),
            ("CustomStringBuildingBlock", "\u{201D}"),
        ],
    ),
    (
        "ufmt3",
        "Paragraph Number & Page Number",
        &[
            ("ParagraphNumberBuildingBlock", "$ID/"),
            ("CustomStringBuildingBlock", " on page "),
            ("PageNumberBuildingBlock", "$ID/"),
        ],
    ),
    (
        "ufmt4",
        "Page Number",
        &[
            ("CustomStringBuildingBlock", "page "),
            ("PageNumberBuildingBlock", "$ID/"),
        ],
    ),
    (
        "ufmt5",
        "Text Anchor Name & Page Number",
        &[
            ("CustomStringBuildingBlock", "\u{201C}"),
            ("BookmarkNameBuildingBlock", "$ID/"),
            ("CustomStringBuildingBlock", "\u{201D} on page "),
            ("PageNumberBuildingBlock", "$ID/"),
        ],
    ),
];

/// Page 1: `(label, format Self, destination, stale text before the
/// page number, whether the format has a page number)`.
pub const REFERENCES: [(&str, &str, &str, &str, bool); 5] = [
    (
        "Full: ",
        "ufmt1",
        PARAGRAPH_DESTINATION,
        "\u{201C}Old Heading\u{201D} on page ",
        true,
    ),
    (
        "Text: ",
        "ufmt2",
        PARAGRAPH_DESTINATION,
        "\u{201C}Old Heading\u{201D}",
        false,
    ),
    (
        "Number: ",
        "ufmt3",
        PARAGRAPH_DESTINATION,
        "7 on page ",
        true,
    ),
    ("Page: ", "ufmt4", PARAGRAPH_DESTINATION, "page ", true),
    (
        "Anchor: ",
        "ufmt5",
        TEXT_DESTINATION,
        "\u{201C}old anchor\u{201D} on page ",
        true,
    ),
];

/// The stale page number every reference prints.
pub const STALE_PAGE: &str = "9";

pub fn story_id() -> String {
    self_id(SAMPLE, "StoryBody", 0)
}

pub fn source_id(i: usize) -> String {
    format!("uxs{}", i + 1)
}

fn range(b: &mut XmlBuilder, text: &str) {
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
    b.end("CharacterStyleRange");
}

fn paragraph(b: &mut XmlBuilder, style: &str, next_frame: bool) {
    let mut attrs = vec![("AppliedParagraphStyle", style)];
    if next_frame {
        attrs.push(("StartParagraph", "NextFrame"));
    }
    b.start("ParagraphStyleRange", &attrs);
}

fn br(b: &mut XmlBuilder) {
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
    b.empty("Br", &[]);
    b.end("CharacterStyleRange");
}

fn body_story() -> Vec<u8> {
    let id = story_id();
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    for (i, (label, format, _, stale, page)) in REFERENCES.iter().enumerate() {
        paragraph(&mut b, BODY_STYLE, false);
        range(&mut b, label);
        let src = source_id(i);
        b.start(
            "CrossReferenceSource",
            &[
                ("Self", src.as_str()),
                ("AppliedFormat", format),
                ("Name", src.as_str()),
                ("Hidden", "false"),
                ("AppliedCharacterStyle", "n"),
            ],
        );
        range(&mut b, stale);
        if *page {
            let tv = format!("{src}tv");
            b.start(
                "CharacterStyleRange",
                &[
                    ("AppliedCharacterStyle", NO_CHAR),
                    ("PageNumberType", "TextVariable"),
                ],
            );
            // Unescaped here: the builder escapes `<` and `>` as InDesign does.
            b.empty(
                "TextVariableInstance",
                &[
                    ("Self", tv.as_str()),
                    ("Name", "<?AID 001b?>TV XRefPageNumber"),
                    ("ResultText", STALE_PAGE),
                    (
                        "AssociatedTextVariable",
                        "dTextVariablen<?AID 001b?>TV XRefPageNumber",
                    ),
                ],
            );
            b.end("CharacterStyleRange");
        }
        b.end("CrossReferenceSource");
        br(&mut b);
        b.end("ParagraphStyleRange");
    }
    // Page 2: the numbered heading, its destination before its text; then
    // the anchor word.
    paragraph(&mut b, HEADING_STYLE, true);
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
    b.empty(
        "ParagraphDestination",
        &[
            ("Self", PARAGRAPH_DESTINATION),
            ("Name", "Anchor 1"),
            ("Hidden", "false"),
            ("DestinationUniqueKey", "1"),
        ],
    );
    b.start("Content", &[]);
    b.text("Where Things Are");
    b.end("Content");
    b.empty("Br", &[]);
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    paragraph(&mut b, BODY_STYLE, false);
    range(&mut b, "The ");
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
    b.empty(
        "HyperlinkTextDestination",
        &[
            ("Self", TEXT_DESTINATION),
            ("Name", "the anchor"),
            ("Hidden", "false"),
            ("DestinationUniqueKey", "2"),
        ],
    );
    b.end("CharacterStyleRange");
    range(&mut b, "anchored word sits here.");
    br(&mut b);
    b.end("ParagraphStyleRange");
    paragraph(&mut b, BODY_STYLE, true);
    range(&mut b, "The end, on page three.");
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

fn formats_xml() -> String {
    let mut out = String::new();
    for (id, name, blocks) in FORMATS {
        out.push_str(&format!(
            "<CrossReferenceFormat Self=\"{id}\" Name=\"{}\" AppliedCharacterStyle=\"n\">",
            name.replace('&', "&amp;")
        ));
        for (n, (kind, text)) in blocks.iter().enumerate() {
            out.push_str(&format!(
                "<BuildingBlock Self=\"{id}BuildingBlock{n}\" BlockType=\"{kind}\" \
AppliedCharacterStyle=\"n\" CustomText=\"{text}\" AppliedDelimiter=\"$ID/\" \
IncludeDelimiter=\"false\" />"
            ));
        }
        out.push_str("</CrossReferenceFormat>");
    }
    out
}

fn hyperlinks_xml() -> String {
    let mut out = String::new();
    for (i, (_, _, dest, _, _)) in REFERENCES.iter().enumerate() {
        let key = if *dest == PARAGRAPH_DESTINATION { 1 } else { 2 };
        out.push_str(&format!(
            "<Hyperlink Self=\"uxl{n}\" Name=\"Cross-Reference {n}\" Source=\"{src}\" \
Visible=\"false\" Highlight=\"None\" Width=\"Thin\" BorderStyle=\"Solid\" Hidden=\"false\" \
DestinationUniqueKey=\"{key}\"><Properties><BorderColor type=\"enumeration\">Black</BorderColor>\
<Destination type=\"object\">{dest}</Destination></Properties></Hyperlink>",
            n = i + 1,
            src = source_id(i),
        ));
    }
    out
}

fn splice(xml: Vec<u8>, needle: &str, insert: &str, before: bool) -> Vec<u8> {
    let mut text = String::from_utf8(xml).expect("utf-8");
    let at = if before {
        text.find(needle).expect("needle present")
    } else {
        text.rfind(needle).expect("needle present") + needle.len()
    };
    text.insert_str(at, insert);
    text.into_bytes()
}

fn styles() -> Vec<u8> {
    let style = |id: &str, name: &str, size: f32, leading: f32, extra: &str, props: &str| {
        format!(
            "<ParagraphStyle Self=\"{id}\" Name=\"{name}\" FontStyle=\"Regular\" \
PointSize=\"{size}\" SpaceAfter=\"4\" Hyphenation=\"false\" FillColor=\"Color/Black\"{extra}>\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn>\
<Leading type=\"unit\">{leading}</Leading>\
<AppliedFont type=\"string\">{FONT}</AppliedFont>{props}</Properties></ParagraphStyle>"
        )
    };
    styles_xml_with_raw(&format!(
        "<RootParagraphStyleGroup>{}{}</RootParagraphStyleGroup>",
        style(BODY_STYLE, "Body", 10.0, 12.0, "", ""),
        style(
            HEADING_STYLE,
            "Heading",
            14.0,
            18.0,
            " BulletsAndNumberingListType=\"NumberedList\"",
            "<NumberingExpression type=\"string\">^#.^t</NumberingExpression>"
        ),
    ))
}

fn frame(id: String, story: String, prev: Option<String>, next: Option<String>) -> PageItem {
    Rect {
        self_id: id,
        width_pt: 320.0,
        height_pt: 240.0,
        item_transform: translate(40.0, 30.0),
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

pub fn build() -> Sample {
    let s = SAMPLE;
    let frames: Vec<String> = (0..3).map(|i| self_id(s, "FrameBody", i)).collect();
    let pages: Vec<Vec<PageItem>> = (0..3)
        .map(|i| {
            vec![frame(
                frames[i].clone(),
                story_id(),
                (i > 0).then(|| frames[i - 1].clone()),
                frames.get(i + 1).cloned(),
            )]
        })
        .collect();
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
            (id, xml)
        })
        .collect();
    let stories = vec![(story_id(), body_story())];
    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
        stories: stories.iter().map(|(id, _)| id.clone()).collect(),
    });
    let variable = format!(
        "<TextVariable Self=\"{XREF_PAGE_VARIABLE}\" Name=\"{XREF_PAGE_NAME}\" \
VariableType=\"XrefPageNumberType\" />"
    );
    let designmap = splice(designmap, "<idPkg:Spread", &variable, true);
    let designmap = splice(designmap, "<idPkg:Story", &formats_xml(), true);
    let designmap = splice(designmap, "</Document>", &hyperlinks_xml(), true);
    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
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
