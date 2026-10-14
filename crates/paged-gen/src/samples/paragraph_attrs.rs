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

//! `paragraph-attrs.idml` — paragraph attributes the engine did not read,
//! asked of InDesign (R-22, R-23, A-10): auto leading, single-word
//! justification, the last line's right indent, balanced ragged lines
//! and no-break text.
//!
//! One Letter page, a grid of 126 × 56 pt frames with no inset, each case
//! labelled above its frame. Open Sans 9 pt, hyphenation off.
//!
//! * A — auto leading (no `Leading`): `AutoLeading` absent (InDesign's
//!   120), 100, 150, and 175 with a 14 pt word on the second line.
//! * B — `SingleWordJustification` on a left-justified paragraph whose
//!   first two lines hold one long word each: absent, `FullyJustified`,
//!   `CenterAlign`, `RightAlign`.
//! * C — `LastLineIndent` on a left-justified paragraph: 0, 12, 36; and
//!   36 on a ragged (left-aligned) paragraph.
//! * D — `BalanceRaggedLines` (`FullyBalanced`): a left-aligned paragraph
//!   with a one-word last line without and with it, the same centred with it, and a 14 pt
//!   two-line heading with it.
//! * E — `NoBreak`: a sentence whose "New York City" falls at a line end
//!   without and with `NoBreak` on it, a longer no-break span, and
//!   `SingleWordJustification="LeftAlign"`. (A no-break run wider than the
//!   line sets nothing from it on — InDesign oversets the rest — which the
//!   engine does not model; ADR 153.)

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "paragraph-attrs";
const PAGE_W_PT: f32 = 612.0;
const PAGE_H_PT: f32 = 792.0;
const FONT: &str = "Open Sans";
const SIZE: f32 = 9.0;
const LEADING: f32 = 11.0;
const LABEL_SIZE: f32 = 6.0;
pub const COLUMNS: [f32; 4] = [36.0, 174.0, 312.0, 450.0];
pub const ROW_TOP: f32 = 72.0;
pub const ROW_PITCH: f32 = 110.0;
pub const FRAME_W: f32 = 126.0;
const FRAME_H: f32 = 90.0;

const LONG_WORDS: &str = "Typographically internationalization extraordinarily done";
const SHORT_LAST: &str =
    "The quick brown fox jumps over the lazy dog while five boxing wizards watch it go";
const CITY: &str = "We will all meet in New York City on Friday";
/// [`SHORT_LAST`] and one word more, which a ragged paragraph sets alone
/// on a fourth line.
const ONE_OVER: &str =
    "The quick brown fox jumps over the lazy dog while five boxing wizards watch it go home";

/// One case: its label, its runs and its paragraph attributes.
struct Case {
    label: &'static str,
    runs: Vec<Run>,
    justification: &'static str,
    leading: Option<f32>,
    attrs: Vec<(&'static str, &'static str)>,
    /// Typed `<Properties>` children, `(name, type, value)`.
    props: Vec<(&'static str, &'static str, &'static str)>,
}

fn run(text: &str, size: f32) -> Run {
    Run {
        extra_char_attrs: Vec::new(),
        text: text.to_string(),
        point_size: Some(size),
        fill_color: Some("Color/Black".to_string()),
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: Some(FONT),
        anchored_frame: None,
    }
}

fn plain(label: &'static str, text: &str, justification: &'static str) -> Case {
    Case {
        label,
        runs: vec![run(text, SIZE)],
        justification,
        leading: Some(LEADING),
        attrs: Vec::new(),
        props: Vec::new(),
    }
}

/// `BalanceRaggedLines` is a typed `<Properties>` child, an enumeration
/// (`NoBalancing`, `FullyBalanced`, `PyramidShape`, `VeeShape`), never an
/// attribute: InDesign ignored `BalanceRaggedLines="true"`.
fn balanced(mut c: Case) -> Case {
    c.props = vec![("BalanceRaggedLines", "enumeration", "FullyBalanced")];
    c
}

fn with(mut c: Case, attrs: Vec<(&'static str, &'static str)>) -> Case {
    c.attrs = attrs;
    c
}

fn auto(label: &'static str, attrs: Vec<(&'static str, &'static str)>) -> Case {
    Case {
        leading: None,
        ..with(plain(label, SHORT_LAST, "LeftAlign"), attrs)
    }
}

fn no_break(text: &str, phrase: &str) -> Vec<Run> {
    let (before, after) = text.split_once(phrase).expect("phrase in text");
    let mut held = run(phrase, SIZE);
    held.extra_char_attrs = vec![("NoBreak", "true")];
    vec![run(before, SIZE), held, run(after, SIZE)]
}

/// The rows, top to bottom; four cases each.
fn rows() -> Vec<[Case; 4]> {
    vec![
        [
            auto("A1 auto leading, default", Vec::new()),
            auto("A2 AutoLeading 100", vec![("AutoLeading", "100")]),
            auto("A3 AutoLeading 150", vec![("AutoLeading", "150")]),
            Case {
                runs: vec![
                    run("The quick brown fox jumps over the ", SIZE),
                    run("lazy", 14.0),
                    run(" dog while five boxing wizards watch it go", SIZE),
                ],
                ..auto(
                    "A4 AutoLeading 175, a 14 pt word",
                    vec![("AutoLeading", "175")],
                )
            },
        ],
        [
            plain("B1 single word, default", LONG_WORDS, "LeftJustified"),
            with(
                plain("B2 single word FullyJustified", LONG_WORDS, "LeftJustified"),
                vec![("SingleWordJustification", "FullyJustified")],
            ),
            with(
                plain("B3 single word CenterAlign", LONG_WORDS, "LeftJustified"),
                vec![("SingleWordJustification", "CenterAlign")],
            ),
            with(
                plain("B4 single word RightAlign", LONG_WORDS, "LeftJustified"),
                vec![("SingleWordJustification", "RightAlign")],
            ),
        ],
        [
            plain("C1 LastLineIndent 0", SHORT_LAST, "LeftJustified"),
            with(
                plain("C2 LastLineIndent 12", SHORT_LAST, "LeftJustified"),
                vec![("LastLineIndent", "12")],
            ),
            with(
                plain("C3 LastLineIndent 36", SHORT_LAST, "LeftJustified"),
                vec![("LastLineIndent", "36")],
            ),
            with(
                plain("C4 LastLineIndent 36, ragged", SHORT_LAST, "LeftAlign"),
                vec![("LastLineIndent", "36")],
            ),
        ],
        [
            plain("D1 ragged", ONE_OVER, "LeftAlign"),
            balanced(plain("D2 BalanceRaggedLines", ONE_OVER, "LeftAlign")),
            balanced(plain(
                "D3 BalanceRaggedLines, centred",
                ONE_OVER,
                "CenterAlign",
            )),
            balanced(Case {
                runs: vec![run("A short heading over two lines of type", 14.0)],
                leading: Some(17.0),
                ..plain("D4 BalanceRaggedLines, 14 pt heading", "", "LeftAlign")
            }),
        ],
        [
            plain("E1 New York City", CITY, "LeftAlign"),
            Case {
                runs: no_break(CITY, "New York City"),
                ..plain("E2 NoBreak New York City", "", "LeftAlign")
            },
            Case {
                runs: no_break(CITY, "all meet in New York"),
                ..plain("E3 NoBreak, a longer span", "", "LeftAlign")
            },
            with(
                plain("E4 single word LeftAlign", LONG_WORDS, "LeftJustified"),
                vec![("SingleWordJustification", "LeftAlign")],
            ),
        ],
    ]
}

fn paragraph(c: Case) -> Paragraph {
    let mut attrs = vec![("Hyphenation", "false")];
    attrs.extend(c.attrs.iter().copied());
    Paragraph {
        extra_paragraph_attrs: attrs,
        leading: c.leading,
        extra_paragraph_props: c.props,
        justification: Some(c.justification),
        runs: c.runs,
        ..Paragraph::plain("")
    }
}

fn label(text: &str) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(LABEL_SIZE * 1.2),
        justification: Some("LeftAlign"),
        runs: vec![run(text, LABEL_SIZE)],
        ..Paragraph::plain("")
    }
}

fn frame(id: String, story: String, size: (f32, f32), at: (f32, f32)) -> PageItem {
    Rect {
        self_id: id,
        width_pt: size.0,
        height_pt: size.1,
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
        text_frame_pref: Some(TextFramePref {
            inset_spacing: Some([0.0; 4]),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

fn story(stories: &mut Vec<(String, Vec<u8>)>, id: String, paragraph: Paragraph) -> String {
    stories.push((
        id.clone(),
        write_story(&Story {
            extra_story_attrs: Vec::new(),
            self_id: id.clone(),
            paragraphs: vec![paragraph],
        }),
    ));
    id
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut items = Vec::new();
    for (r, row) in rows().into_iter().enumerate() {
        for (c, case) in row.into_iter().enumerate() {
            let n = (r * 4 + c) as u32;
            let (x, y) = (COLUMNS[c], ROW_TOP + r as f32 * ROW_PITCH);
            let l = story(
                &mut stories,
                self_id(SAMPLE, "LabelStory", n),
                label(case.label),
            );
            items.push(frame(
                self_id(SAMPLE, "LabelFrame", n),
                l,
                (FRAME_W, 9.0),
                (x, y - 10.0),
            ));
            let b = story(&mut stories, self_id(SAMPLE, "Story", n), paragraph(case));
            items.push(frame(
                self_id(SAMPLE, "Frame", n),
                b,
                (FRAME_W, FRAME_H),
                (x, y),
            ));
        }
    }
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", 0),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        )],
        spreads: vec![(
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id,
                page_self_id: self_id(SAMPLE, "Page", 0),
                page_name: "1".to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        )],
        stories,
    }
}
