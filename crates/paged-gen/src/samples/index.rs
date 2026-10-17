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

//! `index.idml` — an index to generate, asked of InDesign (core
//! ADR 168).
//!
//! Four pages. A threaded body story runs over pages 1-3, one paragraph
//! a page, with page references to topics spelled as InDesign 20.0.1
//! writes them (`index-probe`): `Apples` twice on page 1 and once on page
//! 3, `Cherries` on page 1 with the subtopic `Sour` on page 2, `Bananas`
//! on page 2, `Plums` on pages 2 and 3, and `Fruit`, which has no page of
//! its own but a See reference to `Apples`. The topics nest in the
//! design map's `<Index>`; `<IndexOptions>` in `Preferences.xml` is
//! InDesign's default.
//!
//! Page 4 holds a story with a stale index, which prints as it is (the
//! gate's reference). What InDesign writes on Generate Index is recorded
//! by `tools/indesign-export/index-probe.sh` against this file.

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

pub const SAMPLE: &str = "index";
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

/// The topics: `(id suffix, name, parent id suffix)`.
pub const TOPICS: [(&str, &str, Option<&str>); 6] = [
    ("Apples", "Apples", None),
    ("Bananas", "Bananas", None),
    ("Cherries", "Cherries", None),
    ("CherriesTopicnSour", "Sour", Some("Cherries")),
    ("Fruit", "Fruit", None),
    ("Plums", "Plums", None),
];

/// The body, one paragraph a page; each `[Topic]` before a word is a page
/// reference to that topic, set where the word starts.
pub const BODY: [&str; 3] = [
    "[Apples]Apples and [Cherries]cherries grow here. [Apples]Apples again.",
    "[Bananas]Bananas and [CherriesTopicnSour]sour cherries. [Plums]Plums too.",
    "[Apples]Apples once more. [Plums]Plums and plums.",
];

pub fn topic_id(suffix: &str) -> String {
    format!("u135Topicn{suffix}")
}

pub fn story_id(name: &str) -> String {
    self_id(SAMPLE, &format!("Story{name}"), 0)
}

fn content(b: &mut XmlBuilder, text: &str) {
    if text.is_empty() {
        return;
    }
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
}

fn body_story() -> Vec<u8> {
    let id = story_id("Body");
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    let mut reference = 0u32;
    for (i, raw) in BODY.iter().enumerate() {
        let mut attrs = vec![("AppliedParagraphStyle", BODY_STYLE)];
        if i > 0 {
            attrs.push(("StartParagraph", "NextFrame"));
        }
        b.start("ParagraphStyleRange", &attrs);
        // Each reference in a range of its own, as InDesign writes them.
        let mut rest = *raw;
        while let Some(open) = rest.find('[') {
            if open > 0 {
                b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
                content(&mut b, &rest[..open]);
                b.end("CharacterStyleRange");
            }
            let close = rest[open..].find(']').expect("closed") + open;
            let topic = topic_id(&rest[open + 1..close]);
            reference += 1;
            let self_ref = self_id(SAMPLE, "PageReference", reference);
            let n = reference.to_string();
            b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
            b.empty(
                "PageReference",
                &[
                    ("Self", self_ref.as_str()),
                    ("PageReferenceType", "CurrentPage"),
                    ("ReferencedTopic", topic.as_str()),
                    ("Id", n.as_str()),
                ],
            );
            b.end("CharacterStyleRange");
            rest = &rest[close + 1..];
        }
        b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
        content(&mut b, rest);
        if i + 1 < BODY.len() {
            b.empty("Br", &[]);
        }
        b.end("CharacterStyleRange");
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// The stale index on page 4: what an earlier Generate Index left.
pub const STALE: [&str; 2] = ["Index", "Apples  9"];

fn index_story() -> Vec<u8> {
    let id = story_id("Index");
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id.as_str())]);
    for (i, text) in STALE.iter().enumerate() {
        b.start(
            "ParagraphStyleRange",
            &[("AppliedParagraphStyle", BODY_STYLE)],
        );
        b.start("CharacterStyleRange", &[("AppliedCharacterStyle", NO_CHAR)]);
        content(&mut b, text);
        if i + 1 < STALE.len() {
            b.empty("Br", &[]);
        }
        b.end("CharacterStyleRange");
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// The design map's `<Index>`, as InDesign writes one: a subtopic nests
/// inside its parent, a See reference inside its topic. It stands before
/// the story includes.
fn index_element() -> String {
    let mut out = String::from("<Index Self=\"u135\">");
    for (suffix, name, parent) in TOPICS {
        if parent.is_some() {
            continue;
        }
        let id = topic_id(suffix);
        let kids: Vec<_> = TOPICS
            .iter()
            .filter(|(_, _, p)| *p == Some(suffix))
            .collect();
        let see = (suffix == "Fruit").then(|| {
            format!(
                "<CrossReference Self=\"u159\" ReferencedTopic=\"{}\" \
CrossReferenceType=\"See\" CustomTypeString=\"\" />",
                topic_id("Apples")
            )
        });
        if kids.is_empty() && see.is_none() {
            out.push_str(&format!(
                "<Topic Self=\"{id}\" SortOrder=\"\" Name=\"{name}\" />"
            ));
            continue;
        }
        out.push_str(&format!(
            "<Topic Self=\"{id}\" SortOrder=\"\" Name=\"{name}\">"
        ));
        for (kid, kid_name, _) in kids {
            out.push_str(&format!(
                "<Topic Self=\"{}\" SortOrder=\"\" Name=\"{kid_name}\" />",
                topic_id(kid)
            ));
        }
        if let Some(see) = see {
            out.push_str(&see);
        }
        out.push_str("</Topic>");
    }
    out.push_str("</Index>");
    out
}

/// InDesign's default `<IndexOptions>`, as it writes them.
const INDEX_OPTIONS: &str = "<IndexOptions Title=\"Index\" \
TitleStyle=\"ParagraphStyle/$ID/Index Title\" ReplaceExistingIndex=\"true\" \
IncludeBookDocuments=\"false\" IncludeHiddenEntries=\"false\" IndexFormat=\"NestedFormat\" \
IncludeSectionHeadings=\"true\" IncludeEmptyIndexSections=\"false\" \
Level1Style=\"ParagraphStyle/$ID/Index Level 1\" Level2Style=\"ParagraphStyle/$ID/Index Level 2\" \
Level3Style=\"ParagraphStyle/$ID/[No paragraph style]\" \
Level4Style=\"ParagraphStyle/$ID/[No paragraph style]\" \
SectionHeadingStyle=\"ParagraphStyle/$ID/Index Section Head\" \
PageNumberStyle=\"CharacterStyle/$ID/[No character style]\" \
CrossReferenceStyle=\"CharacterStyle/$ID/Index Cross-reference\" \
CrossReferenceTopicStyle=\"CharacterStyle/$ID/[No character style]\" \
FollowingTopicSeparator=\"  \" BetweenEntriesSeparator=\"; \" PageRangeSeparator=\"^=\" \
BetweenPageNumbersSeparator=\", \" BeforeCrossReferenceSeparator=\". \" EntryEndSeparator=\"\" />";

fn splice_before(xml: Vec<u8>, needle: &str, insert: &str) -> Vec<u8> {
    let mut text = String::from_utf8(xml).expect("utf-8");
    let at = text.find(needle).expect("needle present");
    text.insert_str(at, insert);
    text.into_bytes()
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
        self_id(s, "FrameIndex", 0),
        story_id("Index"),
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
        (story_id("Index"), index_story()),
    ];
    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
        stories: stories.iter().map(|(id, _)| id.clone()).collect(),
    });
    Sample {
        container_xml: container_xml(),
        designmap_xml: splice_before(designmap, "<idPkg:Story", &index_element()),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: splice_before(preferences_xml(), "</idPkg:Preferences>", INDEX_OPTIONS),
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
