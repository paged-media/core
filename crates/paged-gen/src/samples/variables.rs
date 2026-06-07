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

//! W1.18 + W1.19 mega-file: `variables.idml`.
//!
//! Exercises the LIVE variable / cross-reference resolution the
//! `markers` sample deliberately leaves out:
//!
//!   * a **CreationDate** variable with an explicit `Format`
//!     ("MMMM d, yyyy") — formatted from the injectable document clock,
//!     not the stale baked `ResultText`.
//!   * a **ChapterNumber** variable — resolves from the `<Section>`'s
//!     numbering (UpperRoman, start 2 → "II").
//!   * a **RunningHeader** variable on a shared MASTER frame — picks up
//!     the nearest `ParagraphStyle/Heading` paragraph ON ITS PAGE, so
//!     page 1's header reads "Chapter One" and page 2's reads
//!     "Chapter Two" (the multi-page boundary proof).
//!   * a **cross-reference** source whose destination is the page-2
//!     heading story — resolves to the page that story landed on (flat
//!     index 1) AFTER layout.
//!
//! Two A4 pages, each applying the same master (which carries the
//! running-header frame). Each page hosts its own body story with a
//! `Heading`-styled paragraph; page 1 additionally carries the date /
//! chapter / xref body text.

use crate::builders::designmap::{
    write_designmap_with_markers, DesignMap, HyperlinkDef, HyperlinkDestinationDef,
    MarkerResources, SectionDef, TextVariableDef,
};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml,
};
use crate::builders::spread::{write_spread, Spread};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "variables";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 360.0;
const HEADER_W_PT: f32 = 480.0;
const HEADER_H_PT: f32 = 40.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

const HEADING_STYLE: &str = "ParagraphStyle/Heading";
const NO_PARA_STYLE: &str = "ParagraphStyle/$ID/[No paragraph style]";
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

/// One run segment in a body paragraph: plain text, a text-variable
/// instance, or a cross-reference source span.
enum Seg {
    Text(&'static str),
    Variable {
        result_text: &'static str,
        associated: String,
    },
    Xref {
        source_self: String,
        text: &'static str,
    },
}

fn write_seg(b: &mut XmlBuilder, seg: &Seg) {
    let char_style = ("AppliedCharacterStyle", NO_CHAR_STYLE);
    match seg {
        Seg::Text(t) => {
            b.start("CharacterStyleRange", &[char_style, ("PointSize", "16")]);
            b.start("Content", &[]);
            b.text(t);
            b.end("Content");
            b.end("CharacterStyleRange");
        }
        Seg::Variable {
            result_text,
            associated,
        } => {
            b.start("CharacterStyleRange", &[char_style, ("PointSize", "16")]);
            b.empty(
                "TextVariableInstance",
                &[
                    ("ResultText", result_text),
                    ("AssociatedTextVariable", associated.as_str()),
                ],
            );
            b.end("CharacterStyleRange");
        }
        Seg::Xref { source_self, text } => {
            // A cross-reference SOURCE wraps the character range(s) it
            // covers, exactly like a hyperlink source span — the parser
            // inherits the source id onto each enclosed run.
            b.start(
                "CrossReferenceSource",
                &[
                    ("Self", source_self.as_str()),
                    ("Name", source_self.as_str()),
                    ("AppliedCharacterStyle", NO_CHAR_STYLE),
                ],
            );
            b.start("CharacterStyleRange", &[char_style, ("PointSize", "16")]);
            b.start("Content", &[]);
            b.text(text);
            b.end("Content");
            b.end("CharacterStyleRange");
            b.end("CrossReferenceSource");
        }
    }
}

/// A single-paragraph story whose paragraph carries `style` and whose
/// body is `segs`.
fn write_story_paragraph(story_id: &str, style: &str, segs: &[Seg]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    b.start("ParagraphStyleRange", &[("AppliedParagraphStyle", style)]);
    for seg in segs {
        write_seg(&mut b, seg);
    }
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// A two-paragraph body story: a `Heading`-styled paragraph (the
/// running-header pickup target) followed by a body paragraph carrying
/// `body_segs` (which may be empty for the page-2 heading-only story).
fn write_heading_plus_body(story_id: &str, heading: &str, body_segs: &[Seg]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    // Heading paragraph — styled `ParagraphStyle/Heading` so the
    // running-header variable matches it.
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", HEADING_STYLE)],
    );
    b.start(
        "CharacterStyleRange",
        &[
            ("AppliedCharacterStyle", NO_CHAR_STYLE),
            ("PointSize", "24"),
        ],
    );
    b.start("Content", &[]);
    b.text(heading);
    b.end("Content");
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    // Body paragraph.
    if !body_segs.is_empty() {
        b.start(
            "ParagraphStyleRange",
            &[("AppliedParagraphStyle", NO_PARA_STYLE)],
        );
        for seg in body_segs {
            write_seg(&mut b, seg);
        }
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// Build a standard text-frame Rect at `(dx, dy)`.
fn text_frame(self_id: String, w: f32, h: f32, dx: f32, dy: f32, story: String) -> Rect {
    Rect {
        self_id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(dx, dy),
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
        text_frame_pref: None,
        custom_subpaths: None,
    }
}

pub fn build() -> Sample {
    build_with(false)
}

/// W1.19 variant — the same document with a BLANK spread inserted
/// between page 1 and the destination page, so the cross-reference's
/// destination story moves from flat page index 1 to index 2. Proves a
/// fresh render re-resolves the xref against the CURRENT layout rather
/// than a parse-time string.
pub fn build_moved() -> Sample {
    build_with(true)
}

fn build_with(move_destination: bool) -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);

    // Bare story ids — the same convention `markers` uses: the frame's
    // `parent_story`, the designmap `<idPkg:Story>` ref, and the Sample
    // `stories` filename all key off this bare id.
    let body0_story = self_id(SAMPLE, "Story", 0);
    let body1_story = self_id(SAMPLE, "Story", 1);
    let header_story = self_id(SAMPLE, "Story", 2);

    let body0_frame = self_id(SAMPLE, "TextFrame", 0);
    let body1_frame = self_id(SAMPLE, "TextFrame", 1);
    let header_frame = self_id(SAMPLE, "TextFrame", 2);

    let spread0_id = self_id(SAMPLE, "Spread", 0);
    let spread1_id = self_id(SAMPLE, "Spread", 1);
    let page0_id = self_id(SAMPLE, "Page", 0);
    let page1_id = self_id(SAMPLE, "Page", 1);

    let creation_var = format!("TextVariable/{}", self_id(SAMPLE, "TextVariable", 0));
    let chapter_var = format!("TextVariable/{}", self_id(SAMPLE, "TextVariable", 1));
    let header_var = format!("TextVariable/{}", self_id(SAMPLE, "TextVariable", 2));
    let output_var = format!("TextVariable/{}", self_id(SAMPLE, "TextVariable", 3));
    let xref_source = format!("CrossReferenceSource/{}", self_id(SAMPLE, "Xref", 0));
    let xref_dest = format!(
        "HyperlinkTextDestination/{}",
        self_id(SAMPLE, "XrefDest", 0)
    );

    let markers = MarkerResources {
        text_variables: vec![
            TextVariableDef {
                self_id: creation_var.clone(),
                name: "Created".to_string(),
                variable_type: "CreationDateType".to_string(),
                date_format: Some("MMMM d, yyyy".to_string()),
                ..Default::default()
            },
            TextVariableDef {
                self_id: chapter_var.clone(),
                name: "Chapter".to_string(),
                variable_type: "ChapterNumberType".to_string(),
                ..Default::default()
            },
            TextVariableDef {
                self_id: header_var.clone(),
                name: "Running Header".to_string(),
                variable_type: "RunningHeaderType".to_string(),
                running_header_style: Some(HEADING_STYLE.to_string()),
                running_header_use: Some("FirstOnPage".to_string()),
                ..Default::default()
            },
            TextVariableDef {
                self_id: output_var.clone(),
                name: "Output".to_string(),
                variable_type: "OutputDateType".to_string(),
                // 2-digit-everything pattern so the output instant is
                // unambiguous in the glyph stream (yyyy-MM-dd).
                date_format: Some("yyyy-MM-dd".to_string()),
                ..Default::default()
            },
        ],
        hyperlink_destinations: vec![HyperlinkDestinationDef::TextAnchor {
            self_id: xref_dest.clone(),
            // The cross-reference targets the page-2 heading story.
            story: body1_story.clone(),
        }],
        hyperlinks: vec![HyperlinkDef {
            // A <Hyperlink> ties the CrossReferenceSource span to the
            // text destination (InDesign serialises xrefs through the
            // hyperlink machinery).
            self_id: format!("Hyperlink/{}", self_id(SAMPLE, "Hyperlink", 0)),
            name: "xref".to_string(),
            source: xref_source.clone(),
            destination: xref_dest,
        }],
        sections: vec![SectionDef {
            self_id: format!("Section/{}", self_id(SAMPLE, "Section", 0)),
            page_start: page0_id.clone(),
            number_style: Some("UpperRoman".to_string()),
            // Chapter "II" — proves the styled chapter number (not "1").
            start_at: Some(2),
            ..Default::default()
        }],
        footnote_option: None,
    };

    // Page-1 body: heading + a body paragraph with the date, chapter,
    // and a cross-reference whose destination is the page-2 story.
    let page0_body_segs = vec![
        Seg::Text("Created "),
        Seg::Variable {
            result_text: "BAKED-DATE",
            associated: creation_var,
        },
        Seg::Text(". Chapter "),
        Seg::Variable {
            result_text: "BAKED-CH",
            associated: chapter_var,
        },
        Seg::Text(". Output "),
        Seg::Variable {
            result_text: "BAKED-OUT",
            associated: output_var,
        },
        Seg::Text(". See "),
        Seg::Xref {
            source_self: xref_source,
            text: "the next chapter",
        },
        Seg::Text("."),
    ];

    let body0_bytes = write_heading_plus_body(&body0_story, "Chapter One", &page0_body_segs);
    let body1_bytes = write_heading_plus_body(&body1_story, "Chapter Two", &[]);
    // The running-header frame's story: a single paragraph holding the
    // RunningHeader variable. Lives on the master so both pages share it.
    let header_segs = vec![Seg::Variable {
        result_text: "BAKED-HDR",
        associated: header_var,
    }];
    let header_bytes = write_story_paragraph(&header_story, NO_PARA_STYLE, &header_segs);

    // Master carries the running-header frame near the top of the page.
    let header_rect = text_frame(
        header_frame,
        HEADER_W_PT,
        HEADER_H_PT,
        (PAGE_W_PT - HEADER_W_PT) * 0.5,
        40.0,
        header_story.clone(),
    );
    let master_bytes = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![header_rect.into()],
    });

    // Body frames, one per page, below the header.
    let body0_rect = text_frame(
        body0_frame,
        FRAME_W_PT,
        FRAME_H_PT,
        (PAGE_W_PT - FRAME_W_PT) * 0.5,
        140.0,
        body0_story.clone(),
    );
    let body1_rect = text_frame(
        body1_frame,
        FRAME_W_PT,
        FRAME_H_PT,
        (PAGE_W_PT - FRAME_W_PT) * 0.5,
        140.0,
        body1_story.clone(),
    );

    let spread0 = write_spread(&Spread {
        self_id: spread0_id.clone(),
        page_self_id: page0_id,
        page_name: "variables · page 1".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![body0_rect.into()],
        override_list: Vec::new(),
        margins: None,
    });
    let spread1 = write_spread(&Spread {
        self_id: spread1_id.clone(),
        page_self_id: page1_id,
        page_name: "variables · page 2".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![body1_rect.into()],
        override_list: Vec::new(),
        margins: None,
    });

    // W1.19 — the "moved destination" variant inserts a BLANK spread
    // between page 1 and the destination page, so the destination story
    // (and its xref target) shifts one page later. Built only when asked.
    let blank_spread_id = self_id(SAMPLE, "Spread", 2);
    let blank_page_id = self_id(SAMPLE, "Page", 2);
    let blank_spread = move_destination.then(|| {
        write_spread(&Spread {
            self_id: blank_spread_id.clone(),
            page_self_id: blank_page_id,
            page_name: "variables · spacer".to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
            override_list: Vec::new(),
            margins: None,
        })
    });

    // Spread order = page order. Insert the blank between spread0 and
    // spread1 in the moved variant.
    let mut dm_spreads = vec![spread0_id.clone()];
    if move_destination {
        dm_spreads.push(blank_spread_id.clone());
    }
    dm_spreads.push(spread1_id.clone());

    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: dm_spreads,
            stories: vec![
                body0_story.clone(),
                body1_story.clone(),
                header_story.clone(),
            ],
        },
        &markers,
    );

    let mut spreads = vec![(spread0_id, spread0)];
    if let Some(blank) = blank_spread {
        spreads.push((blank_spread_id, blank));
    }
    spreads.push((spread1_id, spread1));

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master_bytes)],
        spreads,
        stories: vec![
            (body0_story, body0_bytes),
            (body1_story, body1_bytes),
            (header_story, header_bytes),
        ],
    }
}
