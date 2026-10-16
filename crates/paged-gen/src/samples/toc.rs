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

//! `toc.idml` — a table of contents that is out of date, asked of
//! InDesign (R-08, L-04; core ADR 165).
//!
//! Four pages. A threaded body story runs over pages 1-3 with headings at
//! three levels: H1 "Getting Started" and H2 "First Steps" on page 1, H1
//! "Going Further" and H3 "Aside" on page 2, H2 "Last Words" on page 3.
//! Page 4 holds the TOC story, bound to `TOCStyle/Contents`, whose stored
//! entries are stale: a renamed heading and a wrong page number.
//!
//! The TOC style is spelled as InDesign 20.0.1 writes one (`toc-probe`):
//! each entry's `Name` names the style it collects, its format style is a
//! `<Properties>` child, and its page number stands after the entry
//! behind a tab in the `TOC Page` character style (H1), before it behind
//! a space (H2), or not at all (H3).
//!
//! InDesign prints the stored entries (its PDF is the gate's reference);
//! what it writes on Update Table of Contents is recorded by
//! `tools/indesign-export/toc-probe.sh` against this file.

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

pub const SAMPLE: &str = "toc";
const PAGE_W_PT: f32 = 400.0;
const PAGE_H_PT: f32 = 300.0;
const FONT: &str = "Inter";
pub const TOC_STYLE: &str = "TOCStyle/Contents";

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");
const NO_CHAR: &str = "CharacterStyle/$ID/[No character style]";

/// The body story: `(paragraph style name, text)`. A text starting with
/// `>` starts the next frame.
pub const BODY: [(&str, &str); 10] = [
    ("H1", "Getting Started"),
    ("Body", "The first chapter opens here."),
    ("H2", "First Steps"),
    ("Body", "A few words under the first section."),
    ("H1", ">Going Further"),
    ("Body", "The second chapter, on page two."),
    ("H3", "Aside"),
    ("Body", "A third-level heading no entry numbers."),
    ("H2", ">Last Words"),
    ("Body", "The end, on page three."),
];

/// The stale TOC: `(paragraph style, [(character style, text)])`.
pub const STALE: [(&str, &[(&str, &str)]); 3] = [
    ("TOC Title", &[("", "Contents")]),
    ("TOC 1", &[("", "Getting Started\t"), ("TOC Page", "7")]),
    ("TOC 1", &[("", "An Old Heading\t"), ("TOC Page", "8")]),
];

pub fn story_id(name: &str) -> String {
    self_id(SAMPLE, &format!("Story{name}"), 0)
}

fn paragraph_style(name: &str) -> String {
    format!("ParagraphStyle/{name}")
}

fn range(b: &mut XmlBuilder, char_style: &str, text: &str) {
    let style = if char_style.is_empty() {
        NO_CHAR.to_string()
    } else {
        format!("CharacterStyle/{char_style}")
    };
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", &style)]);
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
    b.end("CharacterStyleRange");
}

fn br(b: &mut XmlBuilder) {
    b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
    b.empty("Br", &[]);
    b.end("CharacterStyleRange");
}

fn body_story() -> Vec<u8> {
    let id = story_id("Body");
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    for (i, (style, raw)) in BODY.iter().enumerate() {
        let (next_frame, text) = match raw.strip_prefix('>') {
            Some(t) => (true, t),
            None => (false, *raw),
        };
        let applied = paragraph_style(style);
        let mut attrs = vec![("AppliedParagraphStyle", applied.as_str())];
        if next_frame {
            attrs.push(("StartParagraph", "NextFrame"));
        }
        b.start("ParagraphStyleRange", &attrs);
        range(&mut b, "", text);
        if i + 1 < BODY.len() {
            br(&mut b);
        }
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

fn toc_story() -> Vec<u8> {
    let id = story_id("Contents");
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start(
        "Story",
        &[("Self", id.as_str()), ("AppliedTOCStyle", TOC_STYLE)],
    );
    for (i, (style, ranges)) in STALE.iter().enumerate() {
        let applied = paragraph_style(style);
        b.start(
            "ParagraphStyleRange",
            &[("AppliedParagraphStyle", applied.as_str())],
        );
        for (char_style, text) in ranges.iter() {
            range(&mut b, char_style, text);
        }
        if i + 1 < STALE.len() {
            br(&mut b);
        }
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// A paragraph style: size, leading and left indent in pt, Inter, and an
/// optional right tab stop.
fn style(name: &str, size: f32, leading: f32, indent: f32, tab: Option<f32>) -> String {
    let tabs = tab
        .map(|at| {
            format!(
                "<TabList type=\"list\"><ListItem type=\"record\">\
<Alignment type=\"enumeration\">RightAlign</Alignment>\
<AlignmentCharacter type=\"string\">.</AlignmentCharacter>\
<Leader type=\"string\"></Leader>\
<Position type=\"unit\">{at}</Position></ListItem></TabList>"
            )
        })
        .unwrap_or_default();
    format!(
        "<ParagraphStyle Self=\"ParagraphStyle/{name}\" Name=\"{name}\" \
FontStyle=\"Regular\" PointSize=\"{size}\" LeftIndent=\"{indent}\" \
SpaceAfter=\"4\" Hyphenation=\"false\" FillColor=\"Color/Black\">\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn>\
<Leading type=\"unit\">{leading}</Leading>\
<AppliedFont type=\"string\">{FONT}</AppliedFont>{tabs}</Properties>\
</ParagraphStyle>"
    )
}

/// One `<TOCStyleEntry>` as InDesign writes it.
fn entry(n: u32, name: &str, level: u32, position: &str, separator: &str, number: &str) -> String {
    format!(
        "<TOCStyleEntry Self=\"TOCStyle/ContentsTOCStyleEntry{n}\" Name=\"{name}\" \
Level=\"{level}\" PageNumberPosition=\"{position}\" Separator=\"$ID/{separator}\" \
SortAlphabet=\"false\"><Properties>\
<FormatStyle type=\"object\">ParagraphStyle/TOC {level}</FormatStyle>\
<PageNumberStyle type=\"{kind}\">{number}</PageNumberStyle>\
<SeparatorStyle type=\"string\">$ID/[Same Style]</SeparatorStyle>\
</Properties></TOCStyleEntry>",
        kind = if number.starts_with("$ID/") {
            "string"
        } else {
            "object"
        },
    )
}

fn styles() -> Vec<u8> {
    let fragment = format!(
        "<RootCharacterStyleGroup>\
<CharacterStyle Self=\"CharacterStyle/TOC Page\" Name=\"TOC Page\" PointSize=\"8\"/>\
</RootCharacterStyleGroup>\
<RootParagraphStyleGroup>{}{}{}{}{}{}{}{}</RootParagraphStyleGroup>\
<TOCStyle Self=\"{TOC_STYLE}\" Name=\"Contents\" Title=\"Contents\" \
TitleStyle=\"ParagraphStyle/TOC Title\" RunIn=\"false\" IncludeHidden=\"false\" \
IncludeBookDocuments=\"false\" CreateBookmarks=\"true\" SetStoryDirection=\"Horizontal\" \
NumberedParagraphs=\"ExcludeNumbers\" MakeAnchor=\"false\" RemoveForcedLineBreak=\"false\">\
{}{}{}</TOCStyle>",
        style("Body", 10.0, 12.0, 0.0, None),
        style("H1", 16.0, 20.0, 0.0, None),
        style("H2", 13.0, 16.0, 0.0, None),
        style("H3", 10.0, 12.0, 0.0, None),
        style("TOC Title", 14.0, 18.0, 0.0, None),
        style("TOC 1", 10.0, 12.0, 0.0, Some(320.0)),
        style("TOC 2", 10.0, 12.0, 12.0, None),
        style("TOC 3", 10.0, 12.0, 24.0, None),
        entry(0, "H1", 1, "AfterEntry", "^t", "CharacterStyle/TOC Page"),
        entry(1, "H2", 2, "BeforeEntry", " ", "$ID/[Same Style]"),
        entry(2, "H3", 3, "None", "", "$ID/[Same Style]"),
    );
    styles_xml_with_raw(&fragment)
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
    let body: Vec<String> = (0..3).map(|i| self_id(s, "FrameBody", i)).collect();
    let mut pages: Vec<Vec<PageItem>> = (0..3)
        .map(|i| {
            vec![frame(
                body[i].clone(),
                story_id("Body"),
                (i > 0).then(|| body[i - 1].clone()),
                body.get(i + 1).cloned(),
            )]
        })
        .collect();
    pages.push(vec![frame(
        self_id(s, "FrameContents", 0),
        story_id("Contents"),
        None,
        None,
    )]);
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
    let stories = vec![
        (story_id("Body"), body_story()),
        (story_id("Contents"), toc_story()),
    ];
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
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
