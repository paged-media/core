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

//! W4.8 mega-file: `navigation.idml`.
//!
//! The first generated fixture to populate the document-navigation
//! sub-system end to end. One A4 page hosting two `Heading`-styled
//! paragraphs and body text; the document carries:
//!
//!   * a **TOC style** (`TOCStyle/Contents`) whose single
//!     `<TOCStyleEntry IncludeStyle="ParagraphStyle/Heading">` makes the
//!     two headings feed the table of contents. `Document::resolve_toc`
//!     walks them into ordered TOC entries with page numbers.
//!   * **index markers** — two `<PageReference TopicName="…">` markers
//!     inside the body story (one referencing a `<Topic>` by name, one
//!     by `AppliedTopic`). `Document::resolve_index` groups them by topic
//!     and `build_index_paragraphs` emits the index story.
//!   * **bookmarks** — two `<Bookmark>` anchors pointing at hyperlink
//!     text destinations (round-trip for the editor's Bookmarks panel).
//!   * a **cross-reference** — a `<CrossReferenceSource>` span whose
//!     destination is a text anchor (the second heading's story).
//!
//! Parser round-trip asserts the TOC style + entry, the two index
//! markers + topics, the two bookmarks, and the cross-reference. The
//! render test asserts the resolved index story lists both topic
//! markers and the resolved TOC lists both headings.

use crate::builders::designmap::{
    write_designmap_with_markers, BookmarkDef, DesignMap, HyperlinkDestinationDef, IndexTopicDef,
    MarkerResources,
};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw,
};
use crate::builders::spread::{write_spread, Spread};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "navigation";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 600.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

const HEADING_STYLE: &str = "ParagraphStyle/Heading";
const NO_PARA_STYLE: &str = "ParagraphStyle/$ID/[No paragraph style]";
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

/// Self-ids / labels the fixture defines. Exported so the tests refer
/// to them without re-typing the literals.
pub const TOC_STYLE: &str = "TOCStyle/Contents";
pub const HEADING_ONE: &str = "Getting Started";
pub const HEADING_TWO: &str = "Going Further";
pub const TOPIC_APPLE: &str = "Apples";
pub const TOPIC_PEAR: &str = "Pears";
pub const TOPIC_PEAR_ID: &str = "Topic/Pear";
pub const BOOKMARK_ONE: &str = "Bookmark/Intro";
pub const BOOKMARK_TWO: &str = "Bookmark/Further";

/// Emit one `Heading`-styled paragraph carrying `text` — the TOC pickup
/// target.
fn write_heading(b: &mut XmlBuilder, text: &str) {
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", HEADING_STYLE)],
    );
    b.start(
        "CharacterStyleRange",
        &[
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
            ("PointSize", "20"),
        ],
    );
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
}

/// The body story: two headings (TOC sources), a body paragraph that
/// carries two `<PageReference>` index markers + a cross-reference span.
fn body_story_xml(story_id: &str, xref_source: &str) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);

    write_heading(&mut b, HEADING_ONE);

    // Body paragraph with two index markers and an xref source span.
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", NO_PARA_STYLE)],
    );
    b.start(
        "CharacterStyleRange",
        &[
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
            ("PointSize", "12"),
        ],
    );
    b.start("Content", &[]);
    b.text("The orchard grows apples and pears. ");
    b.end("Content");
    // First index marker — references the topic by inline TopicName.
    b.empty("PageReference", &[("TopicName", TOPIC_APPLE)]);
    // Second index marker — references a <Topic> by AppliedTopic (the
    // resolver pulls the topic's Name from the document table).
    b.empty(
        "PageReference",
        &[("AppliedTopic", TOPIC_PEAR_ID), ("TopicName", TOPIC_PEAR)],
    );
    b.end("CharacterStyleRange");
    // Cross-reference source span — wraps a run, like a hyperlink source.
    b.start(
        "CrossReferenceSource",
        &[
            ("Self", xref_source),
            ("Name", xref_source),
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
        ],
    );
    b.start(
        "CharacterStyleRange",
        &[("AppliedCharacterStyle", NO_CHAR_STYLE)],
    );
    b.start("Content", &[]);
    b.text("See the next section");
    b.end("Content");
    b.end("CharacterStyleRange");
    b.end("CrossReferenceSource");
    b.end("ParagraphStyleRange");

    write_heading(&mut b, HEADING_TWO);

    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// `Resources/Styles.xml` carrying the `Heading` paragraph style and the
/// TOC style whose entry picks up `Heading`-styled paragraphs.
fn styles() -> Vec<u8> {
    let fragment = format!(
        "<RootParagraphStyleGroup>\
<ParagraphStyle Self=\"{HEADING_STYLE}\" Name=\"Heading\" AppliedFont=\"Open Sans\" \
PointSize=\"20\" FillColor=\"Color/Black\"/>\
</RootParagraphStyleGroup>\
<RootTOCStyleGroup>\
<TOCStyle Self=\"{TOC_STYLE}\" Name=\"Contents\" Title=\"Contents\" \
TitleStyle=\"{NO_PARA_STYLE}\">\
<TOCStyleEntry Name=\"Heading\" IncludeStyle=\"{HEADING_STYLE}\" \
FormatStyle=\"{NO_PARA_STYLE}\" Level=\"1\" PageNumber=\"On\" Separator=\"^t\"/>\
</TOCStyle>\
</RootTOCStyleGroup>"
    );
    styles_xml_with_raw(&fragment)
}

pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let story_id = self_id(SAMPLE, "Story", 0);
    let frame_id = self_id(SAMPLE, "TextFrame", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let xref_source = format!("CrossReferenceSource/{}", self_id(SAMPLE, "Xref", 0));
    let xref_dest = format!(
        "HyperlinkTextDestination/{}",
        self_id(SAMPLE, "XrefDest", 0)
    );
    let bookmark_dest = format!(
        "HyperlinkTextDestination/{}",
        self_id(SAMPLE, "BookmarkDest", 0)
    );

    let story_bytes = body_story_xml(&story_id, &xref_source);

    let body_rect = Rect {
        self_id: frame_id,
        width_pt: FRAME_W_PT,
        height_pt: FRAME_H_PT,
        item_transform: translate(
            (PAGE_W_PT - FRAME_W_PT) * 0.5,
            (PAGE_H_PT - FRAME_H_PT) * 0.5,
        ),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story_id.clone()),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    };

    let master_bytes = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });

    let spread_bytes = write_spread(&Spread {
        self_id: spread_id.clone(),
        item_transform: None,
        page_self_id: page_id,
        page_name: "navigation".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![body_rect.into()],
        override_list: Vec::new(),
        margins: None,
    });

    let markers = MarkerResources {
        // The xref + bookmark destinations are text anchors targeting
        // the body story.
        hyperlink_destinations: vec![
            HyperlinkDestinationDef::TextAnchor {
                self_id: xref_dest.clone(),
                story: story_id.clone(),
            },
            HyperlinkDestinationDef::TextAnchor {
                self_id: bookmark_dest.clone(),
                story: story_id.clone(),
            },
        ],
        index_topics: vec![IndexTopicDef {
            self_id: TOPIC_PEAR_ID.to_string(),
            name: TOPIC_PEAR.to_string(),
        }],
        bookmarks: vec![
            BookmarkDef {
                self_id: BOOKMARK_ONE.to_string(),
                name: "Introduction".to_string(),
                destination: bookmark_dest.clone(),
            },
            BookmarkDef {
                self_id: BOOKMARK_TWO.to_string(),
                name: "Further reading".to_string(),
                destination: xref_dest,
            },
        ],
        ..Default::default()
    };

    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: vec![story_id.clone()],
        },
        &markers,
    );

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
        master_spreads: vec![(master_id, master_bytes)],
        spreads: vec![(spread_id, spread_bytes)],
        stories: vec![(story_id, story_bytes)],
    }
}
