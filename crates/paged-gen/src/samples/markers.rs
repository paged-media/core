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

//! Phase-W1.4 mega-file: `markers.idml`.
//!
//! Exercises the marker / variable / link resolution paths the other
//! text samples deliberately leave out:
//!
//!   * a **custom text variable** (`CustomTextType`) — resolves to its
//!     literal `Contents` ("Spring 2026")
//!   * a **page-count variable** (`PageCountType`) — resolves to the
//!     document's real total page count (2)
//!   * a **URL hyperlink** span — `https://paged.media`
//!   * a **page-destination hyperlink** span — jumps to page 2
//!
//! Two A4 pages: page 1 carries the body text with the variables + both
//! link spans; page 2 is the jump target for the page hyperlink. The
//! story is emitted with a purpose-built writer (rather than the generic
//! `story` builder) so it can nest `<TextVariableInstance>` and
//! `<HyperlinkTextSource>` without touching the shared `Run` shape.

use crate::builders::designmap::{
    write_designmap_with_markers, DesignMap, HyperlinkDef, HyperlinkDestinationDef,
    MarkerResources, TextVariableDef,
};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml,
};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "markers";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 400.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

/// One run segment in the markers story. Either a plain text span, a
/// text-variable instance, or a hyperlink-source span (text wrapped in
/// a `<HyperlinkTextSource>`).
enum Seg {
    /// Plain `<Content>` text.
    Text(&'static str),
    /// `<TextVariableInstance ResultText=... AssociatedTextVariable=.../>`.
    Variable {
        result_text: &'static str,
        associated: String,
    },
    /// `<HyperlinkTextSource Self=...>` wrapping the linked text.
    Link {
        source_self: String,
        text: &'static str,
    },
}

/// Emit one CharacterStyleRange whose body is `seg`. Each segment is its
/// own character range so the variable / link wrappers sit at clean run
/// boundaries (which is how InDesign serialises them).
fn write_seg(b: &mut XmlBuilder, seg: &Seg) {
    let char_style = (
        "AppliedCharacterStyle",
        "CharacterStyle/$ID/[No character style]",
    );
    match seg {
        Seg::Text(t) => {
            b.start("CharacterStyleRange", &[char_style, ("PointSize", "18")]);
            b.start("Content", &[]);
            b.text(t);
            b.end("Content");
            b.end("CharacterStyleRange");
        }
        Seg::Variable {
            result_text,
            associated,
        } => {
            b.start("CharacterStyleRange", &[char_style, ("PointSize", "18")]);
            b.empty(
                "TextVariableInstance",
                &[
                    ("ResultText", result_text),
                    ("AssociatedTextVariable", associated.as_str()),
                ],
            );
            b.end("CharacterStyleRange");
        }
        Seg::Link { source_self, text } => {
            // The source span WRAPS the character range(s) it covers —
            // that's how InDesign serialises a hyperlink source, and
            // how the parser inherits the source id onto each enclosed
            // run.
            b.start(
                "HyperlinkTextSource",
                &[
                    ("Self", source_self.as_str()),
                    ("Name", source_self.as_str()),
                    ("Hidden", "false"),
                    (
                        "AppliedCharacterStyle",
                        "CharacterStyle/$ID/[No character style]",
                    ),
                ],
            );
            b.start("CharacterStyleRange", &[char_style, ("PointSize", "18")]);
            b.start("Content", &[]);
            b.text(text);
            b.end("Content");
            b.end("CharacterStyleRange");
            b.end("HyperlinkTextSource");
        }
    }
}

/// Emit a markers story: one paragraph with the given segments.
fn write_markers_story(story_id: &str, segs: &[Seg]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    b.start(
        "ParagraphStyleRange",
        &[(
            "AppliedParagraphStyle",
            "ParagraphStyle/$ID/[No paragraph style]",
        )],
    );
    for seg in segs {
        write_seg(&mut b, seg);
    }
    b.end("ParagraphStyleRange");
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

pub fn build() -> Sample {
    // Stable ids.
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let story_id = self_id(SAMPLE, "Story", 0);
    let frame_id = self_id(SAMPLE, "TextFrame", 0);
    let spread0_id = self_id(SAMPLE, "Spread", 0);
    let spread1_id = self_id(SAMPLE, "Spread", 1);
    let page0_id = self_id(SAMPLE, "Page", 0);
    let page1_id = self_id(SAMPLE, "Page", 1);

    // Marker resource ids.
    let custom_var = format!("TextVariable/{}", self_id(SAMPLE, "TextVariable", 0));
    let pagecount_var = format!("TextVariable/{}", self_id(SAMPLE, "TextVariable", 1));
    let url_source = format!("HyperlinkTextSource/{}", self_id(SAMPLE, "HLSource", 0));
    let page_source = format!("HyperlinkTextSource/{}", self_id(SAMPLE, "HLSource", 1));
    let url_dest = format!(
        "HyperlinkURLDestination/{}",
        self_id(SAMPLE, "HLUrlDest", 0)
    );
    let page_dest = format!(
        "HyperlinkPageDestination/{}",
        self_id(SAMPLE, "HLPageDest", 0)
    );

    let markers = MarkerResources {
        text_variables: vec![
            TextVariableDef {
                self_id: custom_var.clone(),
                name: "Season".to_string(),
                variable_type: "CustomTextType".to_string(),
                contents: Some("Spring 2026".to_string()),
            },
            TextVariableDef {
                self_id: pagecount_var.clone(),
                name: "Page Count".to_string(),
                variable_type: "PageCountType".to_string(),
                contents: None,
            },
        ],
        hyperlink_destinations: vec![
            HyperlinkDestinationDef::Url {
                self_id: url_dest.clone(),
                url: "https://paged.media".to_string(),
            },
            HyperlinkDestinationDef::Page {
                self_id: page_dest.clone(),
                // Target the SECOND page by its <Page Self> id.
                page: page1_id.clone(),
            },
        ],
        hyperlinks: vec![
            HyperlinkDef {
                self_id: format!("Hyperlink/{}", self_id(SAMPLE, "Hyperlink", 0)),
                name: "url-link".to_string(),
                source: url_source.clone(),
                destination: url_dest,
            },
            HyperlinkDef {
                self_id: format!("Hyperlink/{}", self_id(SAMPLE, "Hyperlink", 1)),
                name: "page-link".to_string(),
                source: page_source.clone(),
                destination: page_dest,
            },
        ],
    };

    // The body paragraph: plain text interleaved with a custom
    // variable, a page-count variable, a URL link span, and a
    // page-destination link span.
    let segs = vec![
        Seg::Text("Season: "),
        Seg::Variable {
            result_text: "Spring 2026",
            associated: custom_var,
        },
        Seg::Text(". Pages: "),
        Seg::Variable {
            // Stale baked value (real export said 1) — the renderer
            // re-resolves to the real count (2).
            result_text: "1",
            associated: pagecount_var,
        },
        Seg::Text(". Visit "),
        Seg::Link {
            source_self: url_source,
            text: "paged.media",
        },
        Seg::Text(" or jump to "),
        Seg::Link {
            source_self: page_source,
            text: "the next page",
        },
        Seg::Text("."),
    ];

    let master_bytes = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let story_bytes = write_markers_story(&story_id, &segs);

    let frame_transform = translate(
        (PAGE_W_PT - FRAME_W_PT) * 0.5,
        (PAGE_H_PT - FRAME_H_PT) * 0.33,
    );
    let body_frame = Rect {
        self_id: frame_id,
        width_pt: FRAME_W_PT,
        height_pt: FRAME_H_PT,
        item_transform: frame_transform,
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
    };

    let spread0 = crate::builders::spread::write_spread(&crate::builders::spread::Spread {
        self_id: spread0_id.clone(),
        page_self_id: page0_id,
        page_name: "markers · variables + links".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![body_frame.into()],
        override_list: Vec::new(),
        margins: None,
    });
    // Page 2 — the page-hyperlink jump target. No body items.
    let spread1 = crate::builders::spread::write_spread(&crate::builders::spread::Spread {
        self_id: spread1_id.clone(),
        page_self_id: page1_id,
        page_name: "markers · link target".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
        override_list: Vec::new(),
        margins: None,
    });

    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread0_id.clone(), spread1_id.clone()],
            stories: vec![story_id.clone()],
        },
        &markers,
    );

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
        spreads: vec![(spread0_id, spread0), (spread1_id, spread1)],
        stories: vec![(story_id, story_bytes)],
    }
}
