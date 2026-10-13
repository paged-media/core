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

//! `conditions.idml` — which conditional text prints, asked of InDesign.
//!
//! Three conditions are declared in `designmap.xml`: `Visible`
//! (`Visible="true"`), `Hidden` and `Hidden2` (both `Visible="false"`).
//! One A4 page holds one story; every case is its own numbered paragraph
//! (or table cell) reading `NN label: WORD.`, where only `WORD` carries
//! the case's `AppliedConditions` — so the label and the full stop
//! always print and a hidden case reads `NN label: .`. See [`cases`].
//!
//! # What InDesign answered (20.0.1, `tools/indesign-export/conditions-probe.sh`)
//!
//! * **InDesign does not hide text on open.** A `Visible="false"` flag
//!   is read (the probe reports the condition hidden) but text written
//!   inline under it is COMPOSED and PRINTED. The flag only acts when a
//!   condition's visibility changes. That is why this fixture's first
//!   reference printed the hidden run.
//! * **InDesign's own spelling of hidden text is `<HiddenText>`.** Toggle
//!   a condition in the opened file and export the IDML: every hidden
//!   range moves into `<CharacterStyleRange><HiddenText>…</HiddenText>
//!   </CharacterStyleRange>`, the text keeping its `AppliedConditions`
//!   inside. So this sample writes each case InDesign hides in that
//!   form ([`Spelling::HiddenText`], what [`build`] emits), and InDesign
//!   prints exactly what it printed after the toggle.
//! * **The rule** (the toggled composition): a range under several
//!   conditions prints while ANY of them is visible; it is hidden only
//!   when every condition it names is hidden. A range naming a condition
//!   the document does not declare prints, even beside a hidden one:
//!   InDesign drops that whole `AppliedConditions` list on import (its
//!   export writes the range ungated). The same holds for an anchored
//!   object's marker (the object goes with it), for text in a table cell,
//!   and for a footnote reference (the note goes with it). Each case's
//!   [`Case::prints`] is that answer.
//!
//! A visible footnote reference was asked too (under `Visible` it prints,
//! numbered, with its note). It is not on the page: the renderer does not
//! draw a reference's number yet and the text gate reads no footnote
//! lines, so it would fail both gates for reasons that are not
//! conditional text.
//!
//! [`build_inline`] writes every case inline instead, which is how the
//! engine's model carries conditional text (the importer drops
//! `<HiddenText>`) and how a hand-written file spells it. The renderer
//! applies the flags to it, and must print what InDesign printed.

use crate::builders::designmap::{write_designmap_with_markers, DesignMap, MarkerResources};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::{AnchoredObjectSetting, Rect};
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml, ConditionSetSpec,
    ConditionSpec,
};
use crate::builders::spread::{write_spread, Spread};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::{translate, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "conditions";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_X_PT: f32 = 57.0;
const FRAME_Y_PT: f32 = 57.0;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 700.0;
const POINT_SIZE: &str = "14";
const CELL_W_PT: f32 = 160.0;
const CELL_H_PT: f32 = 28.0;
/// The side of the black square anchored inline in the anchor cases.
pub const ANCHOR_SIDE_PT: f32 = 10.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

const NO_PARA_STYLE: &str = "ParagraphStyle/$ID/[No paragraph style]";
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

pub const CONDITION_VISIBLE: &str = "Condition/Visible";
pub const CONDITION_HIDDEN: &str = "Condition/Hidden";
pub const CONDITION_HIDDEN2: &str = "Condition/Hidden2";
/// Referenced by cases 7 and 8, declared nowhere.
pub const CONDITION_UNKNOWN: &str = "Condition/Undeclared";

/// A `<ConditionSet>` of the two hidden conditions. No set is active, and
/// a set decides nothing by itself, so InDesign prints the page as the
/// conditions' own flags say; activating it (`activateConditionSet`)
/// shows its members and hides `Visible`, so the page changes.
pub const CONDITION_SET: &str = "ConditionSet/PrintPreview";

/// Where a case's conditional range sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A run of body text.
    Text,
    /// A run holding an inline anchored object (a black square) and the
    /// word: the condition covers the anchor marker.
    Anchor,
    /// The text of a table cell.
    Cell,
    /// A run holding the word and a footnote reference: the condition
    /// covers the reference marker.
    Footnote,
}

/// One numbered case.
pub struct Case {
    pub number: u32,
    /// What is applied, as printed on the page.
    pub label: &'static str,
    /// The word the conditions gate.
    pub word: &'static str,
    /// `AppliedConditions`, in this order.
    pub conditions: &'static [&'static str],
    pub place: Place,
    /// Whether InDesign 20.0.1 prints the word (see the module doc).
    pub prints: bool,
}

const fn case(
    number: u32,
    label: &'static str,
    word: &'static str,
    conditions: &'static [&'static str],
    place: Place,
    prints: bool,
) -> Case {
    Case {
        number,
        label,
        word,
        conditions,
        place,
        prints,
    }
}

const V: &str = CONDITION_VISIBLE;
const H: &str = CONDITION_HIDDEN;
const H2: &str = CONDITION_HIDDEN2;
const U: &str = CONDITION_UNKNOWN;

/// The cases, in story order. Cases 11–13 are the three cells of one
/// table.
pub fn cases() -> Vec<Case> {
    use Place::*;
    vec![
        case(1, "ungated", "ALPHA", &[], Text, true),
        case(2, "Visible", "BRAVO", &[V], Text, true),
        case(3, "Hidden", "CHARLIE", &[H], Text, false),
        case(4, "Visible Hidden", "DELTA", &[V, H], Text, true),
        case(5, "Hidden Visible", "ECHO", &[H, V], Text, true),
        case(6, "Hidden Hidden2", "FOXTROT", &[H, H2], Text, false),
        case(7, "undeclared", "GOLF", &[U], Text, true),
        case(8, "Hidden undeclared", "HOTEL", &[H, U], Text, true),
        case(9, "anchor, Hidden", "INDIA", &[H], Anchor, false),
        case(
            10,
            "anchor, Visible Hidden",
            "JULIETT",
            &[V, H],
            Anchor,
            true,
        ),
        case(11, "cell, Hidden", "KILO", &[H], Cell, false),
        case(12, "cell, Visible Hidden", "LIMA", &[V, H], Cell, true),
        case(13, "cell, Hidden Hidden2", "MIKE", &[H, H2], Cell, false),
        case(14, "footnote, Hidden", "NOVEMBER", &[H], Footnote, false),
    ]
}

/// The body text of a footnote whose reference case `n` gates.
pub fn footnote_text(n: u32) -> String {
    format!("note {n:02}")
}

/// How the cases InDesign hides are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spelling {
    /// Inside `<HiddenText>`, the way InDesign writes hidden text.
    HiddenText,
    /// Inline, the conditions alone saying it is hidden.
    Inline,
}

fn char_range(b: &mut XmlBuilder, conditions: &[&str]) {
    let joined = conditions.join(" ");
    let mut attrs: Vec<(&str, &str)> = vec![
        ("AppliedCharacterStyle", NO_CHAR_STYLE),
        ("PointSize", POINT_SIZE),
    ];
    if !conditions.is_empty() {
        attrs.push(("AppliedConditions", joined.as_str()));
    }
    b.start("CharacterStyleRange", &attrs);
}

fn content(b: &mut XmlBuilder, text: &str) {
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
}

/// An ungated run; `mark` ends the paragraph with `<Br/>`.
fn plain_run(b: &mut XmlBuilder, text: &str, mark: bool) {
    char_range(b, &[]);
    content(b, text);
    if mark {
        b.empty("Br", &[]);
    }
    b.end("CharacterStyleRange");
}

fn anchored_square(n: u32) -> Rect {
    Rect {
        self_id: self_id(SAMPLE, "Rectangle", n),
        width_pt: ANCHOR_SIDE_PT,
        height_pt: ANCHOR_SIDE_PT,
        item_transform: IDENTITY,
        fill_color: Some("Color/Black".to_string()),
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: None,
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: Some(AnchoredObjectSetting::inline()),
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    }
}

fn footnote(b: &mut XmlBuilder, n: u32) {
    let id = self_id(SAMPLE, "Footnote", n);
    b.start("Footnote", &[("Self", id.as_str())]);
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", NO_PARA_STYLE)],
    );
    char_range(b, &[]);
    content(b, &footnote_text(n));
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
    b.end("Footnote");
}

/// The gated range itself: the word, plus the anchored square or the
/// footnote reference the case puts under the same conditions.
fn gated_body(b: &mut XmlBuilder, c: &Case) {
    char_range(b, c.conditions);
    if c.place == Place::Anchor {
        anchored_square(c.number).write(b);
    }
    content(b, c.word);
    if c.place == Place::Footnote {
        footnote(b, c.number);
    }
    b.end("CharacterStyleRange");
}

/// The gated range, in `spelling`: a case InDesign hides is wrapped in
/// `<HiddenText>` (inside an ungated range holding a paragraph range of
/// its own, InDesign's shape) unless `Inline` is asked for.
fn gated(b: &mut XmlBuilder, c: &Case, spelling: Spelling) {
    if spelling == Spelling::Inline || c.prints {
        gated_body(b, c);
        return;
    }
    char_range(b, &[]);
    b.start("HiddenText", &[]);
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", NO_PARA_STYLE)],
    );
    gated_body(b, c);
    b.end("ParagraphStyleRange");
    b.end("HiddenText");
    b.end("CharacterStyleRange");
}

fn label(c: &Case) -> String {
    format!("{:02} {}: ", c.number, c.label)
}

fn cell_paragraph(b: &mut XmlBuilder, c: &Case, spelling: Spelling) {
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", NO_PARA_STYLE)],
    );
    plain_run(b, &format!("{:02} ", c.number), false);
    gated(b, c, spelling);
    plain_run(b, ".", false);
    b.end("ParagraphStyleRange");
}

/// One body row of three cells, one per cell case.
fn table(b: &mut XmlBuilder, cells: &[&Case], spelling: Spelling) {
    let id = self_id(SAMPLE, "Table", 0);
    let cols = cells.len().to_string();
    b.start(
        "Table",
        &[
            ("Self", id.as_str()),
            ("HeaderRowCount", "0"),
            ("FooterRowCount", "0"),
            ("BodyRowCount", "1"),
            ("ColumnCount", cols.as_str()),
            ("AppliedTableStyle", "TableStyle/$ID/[No table style]"),
        ],
    );
    let h = crate::xml::format_f32(CELL_H_PT);
    let w = crate::xml::format_f32(CELL_W_PT);
    b.empty(
        "Row",
        &[
            ("Self", &format!("{id}_R0")),
            ("Name", "0"),
            ("SingleRowHeight", &h),
            ("MinimumHeight", &h),
            ("AutoGrow", "true"),
        ],
    );
    for i in 0..cells.len() {
        b.empty(
            "Column",
            &[
                ("Self", &format!("{id}_C{i}")),
                ("Name", &i.to_string()),
                ("SingleColumnWidth", &w),
            ],
        );
    }
    for (i, c) in cells.iter().enumerate() {
        b.start(
            "Cell",
            &[
                ("Self", &format!("{id}_{i}_0")),
                ("Name", &format!("{i}:0")),
                ("RowSpan", "1"),
                ("ColumnSpan", "1"),
            ],
        );
        cell_paragraph(b, c, spelling);
        b.end("Cell");
    }
    b.end("Table");
}

fn body_story_xml(story_id: &str, spelling: Spelling) -> Vec<u8> {
    let all = cases();
    let cells: Vec<&Case> = all.iter().filter(|c| c.place == Place::Cell).collect();
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    // One `<ParagraphStyleRange>` per paragraph, each ending in its `<Br/>`
    // mark but the last: InDesign reads the marks, and the engine's reader
    // then sets the table in its place (a table inside a longer range is
    // set after all of that range's text).
    let mut table_written = false;
    for (i, c) in all.iter().enumerate() {
        let last = i + 1 == all.len();
        if c.place == Place::Cell {
            if !table_written {
                // A label paragraph, then a paragraph holding the table.
                let span = format!(
                    "{:02}-{:02} table cells:",
                    cells[0].number,
                    cells[cells.len() - 1].number
                );
                b.start(
                    "ParagraphStyleRange",
                    &[("AppliedParagraphStyle", NO_PARA_STYLE)],
                );
                plain_run(&mut b, &span, true);
                b.end("ParagraphStyleRange");
                b.start(
                    "ParagraphStyleRange",
                    &[("AppliedParagraphStyle", NO_PARA_STYLE)],
                );
                char_range(&mut b, &[]);
                table(&mut b, &cells, spelling);
                b.empty("Br", &[]);
                b.end("CharacterStyleRange");
                b.end("ParagraphStyleRange");
                table_written = true;
            }
            continue;
        }
        b.start(
            "ParagraphStyleRange",
            &[("AppliedParagraphStyle", NO_PARA_STYLE)],
        );
        plain_run(&mut b, &label(c), false);
        gated(&mut b, c, spelling);
        plain_run(&mut b, ".", !last);
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

/// The fixture: hidden cases in InDesign's `<HiddenText>` spelling.
pub fn build() -> Sample {
    build_with(Spelling::HiddenText)
}

/// The same cases with every range inline (see the module doc).
pub fn build_inline() -> Sample {
    build_with(Spelling::Inline)
}

pub fn build_with(spelling: Spelling) -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let story_id = self_id(SAMPLE, "Story", 0);
    let frame_id = self_id(SAMPLE, "TextFrame", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let story_bytes = body_story_xml(&story_id, spelling);

    let body_rect = Rect {
        self_id: frame_id,
        width_pt: FRAME_W_PT,
        height_pt: FRAME_H_PT,
        item_transform: translate(FRAME_X_PT, FRAME_Y_PT),
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
        page_self_id: page_id,
        page_name: "conditions".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![body_rect.into()],
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    let conditions = vec![
        ConditionSpec {
            self_id: CONDITION_VISIBLE,
            name: "Visible",
            visible: true,
            indicator_color: Some("Green"),
        },
        ConditionSpec {
            self_id: CONDITION_HIDDEN,
            name: "Hidden",
            visible: false,
            indicator_color: Some("Red"),
        },
        ConditionSpec {
            self_id: CONDITION_HIDDEN2,
            name: "Hidden2",
            visible: false,
            indicator_color: Some("Blue"),
        },
    ];
    let condition_sets = vec![ConditionSetSpec {
        self_id: CONDITION_SET,
        name: "Print preview",
        conditions: &[CONDITION_HIDDEN, CONDITION_HIDDEN2],
    }];
    // Conditions live in the designmap (InDesign's spelling), not in
    // Styles.xml — see `ConditionSpec`.
    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: vec![story_id.clone()],
        },
        &MarkerResources {
            conditions,
            condition_sets,
            ..MarkerResources::default()
        },
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
        spreads: vec![(spread_id, spread_bytes)],
        stories: vec![(story_id, story_bytes)],
    }
}
