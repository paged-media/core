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

//! `footnote-references.idml` — where a footnote's REFERENCE stands in
//! body text, asked of InDesign.
//!
//! InDesign keeps the reference as a character of its line (U+0004 in its
//! story): it prints the note's number, takes width, and can move a line
//! break. Its own export writes the reference's formatting on a range of
//! its own around the `<Footnote>` —
//!
//! ```xml
//! <CharacterStyleRange … PointSize="10" Position="Superscript"><Footnote>…</Footnote></CharacterStyleRange>
//! ```
//!
//! — and the note's own number as `<?ACE 4?>` in the note's text, followed
//! by the separator as text (InDesign 20.0.1, notes built through its DOM
//! and exported). `MarkerPositioning` is applied as character formatting
//! when a note is made: a reference in a range that says nothing prints at
//! the range's size, on its baseline (the `footnotes` fixture's references
//! come back `NORMAL`, 12 pt, in a document whose option says
//! `SuperscriptMarker`).
//!
//! One story, one frame, one numbered case per paragraph; the notes are
//! numbered through the story from `StartAt="3"`:
//!
//!  1. mid-sentence, in InDesign's spelling;
//!  2. at the end of the paragraph;
//!  3. two references in one paragraph;
//!  4. a reference inside the text's own range (the engine's older
//!     spelling): full size, on the baseline;
//!  5. a bold reference — its range's formatting;
//!  6. a reference ending a line, before a forced line break.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "footnote-references";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
pub const FRAME_W: f32 = 420.0;
const FRAME_H: f32 = 400.0;
pub const FRAME_ORIGIN: (f32, f32) = (88.0, 110.0);
/// `FootnoteOption StartAt`: the first note's number.
pub const START_AT: u32 = 3;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

/// One piece of a paragraph, in order.
#[derive(Clone, Copy)]
pub enum Piece {
    /// Text in the paragraph's plain range.
    Text(&'static str),
    /// A note whose reference has a range of its own carrying these
    /// attributes (InDesign's spelling), with this note text.
    OwnRange(&'static [(&'static str, &'static str)], &'static str),
    /// A note inside the plain range, with this note text.
    Inline(&'static str),
}

/// What InDesign writes on the range of a reference it made.
const SUPERSCRIPT: &[(&str, &str)] = &[("Position", "Superscript")];
const BOLD_SUPERSCRIPT: &[(&str, &str)] = &[("Position", "Superscript"), ("FontStyle", "Bold")];

/// The paragraphs, in story order. `|` is a forced line break.
pub fn paragraphs() -> Vec<Vec<Piece>> {
    use Piece::{Inline, OwnRange, Text};
    vec![
        vec![
            Text("1 Mid-sentence: alpha beta"),
            OwnRange(SUPERSCRIPT, "Note on beta, mid-sentence."),
            Text(" gamma delta."),
        ],
        vec![
            Text("2 End of paragraph: the reference follows the period."),
            OwnRange(SUPERSCRIPT, "Note at the end of a paragraph."),
        ],
        vec![
            Text("3 Two in one paragraph: first"),
            OwnRange(SUPERSCRIPT, "The first of two."),
            Text(" and second"),
            OwnRange(SUPERSCRIPT, "The second of two."),
            Text(" here."),
        ],
        vec![
            Text("4 In the text's own range, the reference prints at its size"),
            Inline("A reference written inside its text's range."),
            Text(" and on the baseline."),
        ],
        vec![
            Text("5 A bold reference"),
            OwnRange(BOLD_SUPERSCRIPT, "A bold reference."),
            Text(" in a range of its own."),
        ],
        vec![
            Text("6 A reference ends this line"),
            OwnRange(SUPERSCRIPT, "Before a forced line break."),
            Text("|and the paragraph goes on."),
        ],
    ]
}

/// The body story's id.
pub fn body_story_id() -> String {
    self_id(SAMPLE, "BodyStory", 0)
}

/// A range in InDesign's spelling: every one names its font style.
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
    for &(k, v) in extra {
        attrs.retain(|(key, _)| *key != k);
        attrs.push((k, v));
    }
    b.start("CharacterStyleRange", &attrs);
    b.start("Properties", &[]);
    b.start("AppliedFont", &[("type", "string")]);
    b.text(BODY_FONT);
    b.end("AppliedFont");
    b.end("Properties");
}

fn content(b: &mut XmlBuilder, text: &str) {
    b.start("Content", &[]);
    b.text(&text.replace('|', "\u{2028}"));
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

fn story_xml(id: &str) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id)]);
    let paragraphs = paragraphs();
    let last = paragraphs.len() - 1;
    let mut note = 0u32;
    for (pi, pieces) in paragraphs.iter().enumerate() {
        b.start(
            "ParagraphStyleRange",
            &[
                (
                    "AppliedParagraphStyle",
                    "ParagraphStyle/$ID/[No paragraph style]",
                ),
                ("Hyphenation", "false"),
                ("SpaceAfter", "6"),
            ],
        );
        let mut open = false;
        for piece in pieces {
            match piece {
                Piece::Text(t) => {
                    if !open {
                        start_range(&mut b, &[], "10");
                        open = true;
                    }
                    content(&mut b, t);
                }
                Piece::Inline(text) => {
                    if !open {
                        start_range(&mut b, &[], "10");
                        open = true;
                    }
                    footnote(&mut b, self_id(SAMPLE, "Footnote", note), text);
                    note += 1;
                }
                Piece::OwnRange(attrs, text) => {
                    if open {
                        b.end("CharacterStyleRange");
                        open = false;
                    }
                    start_range(&mut b, attrs, "10");
                    footnote(&mut b, self_id(SAMPLE, "Footnote", note), text);
                    note += 1;
                    b.end("CharacterStyleRange");
                }
            }
        }
        if pi < last {
            if !open {
                start_range(&mut b, &[], "10");
                open = true;
            }
            b.empty("Br", &[]);
        }
        if open {
            b.end("CharacterStyleRange");
        }
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// `Resources/Preferences.xml` with the document's `<FootnoteOption>`,
/// where InDesign keeps it, its enumerations as typed `<Properties>`
/// children (InDesign 20.0.1's own spelling).
fn preferences_xml() -> Vec<u8> {
    let start_at = START_AT.to_string();
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Preferences", &[PKG_NS, DOM_VERSION]);
    b.empty("TextPreference", &[("UseOpticalSize", "false")]);
    b.start(
        "FootnoteOption",
        &[
            ("StartAt", start_at.as_str()),
            ("Prefix", ""),
            ("Suffix", ""),
            ("SpaceBetween", "0"),
            ("Spacer", "0"),
            ("RuleOn", "true"),
            ("RuleLineWeight", "1"),
            ("RuleWidth", "72"),
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

fn frame(id: String, story: String, w: f32, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
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
        text_frame_pref: body.then(|| TextFramePref {
            inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
            first_baseline_offset: Some("AscentOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let label_story = self_id(SAMPLE, "LabelStory", 0);
    let body = body_story_id();
    let stories = vec![
        (
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph::plain(
                    "footnote-references: notes numbered from 3",
                )],
            }),
        ),
        (body.clone(), story_xml(&body)),
    ];
    let items = vec![
        frame(
            self_id(SAMPLE, "LabelFrame", 0),
            label_story.clone(),
            FRAME_W,
            20.0,
            (FRAME_ORIGIN.0, FRAME_ORIGIN.1 - 30.0),
            false,
        ),
        frame(
            self_id(SAMPLE, "Frame", 0),
            body.clone(),
            FRAME_W,
            FRAME_H,
            FRAME_ORIGIN,
            true,
        ),
    ];

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: self_id(SAMPLE, "Page", 0),
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
            stories: vec![label_story, body],
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
