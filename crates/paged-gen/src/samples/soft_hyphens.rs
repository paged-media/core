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

//! `soft-hyphens.idml` — the discretionary hyphen (U+00AD, InDesign's
//! Cmd+Shift+-) against InDesign.
//!
//! InDesign spells a discretionary hyphen inside `<Content>` as the bare
//! U+00AD (asked 2026-10-02: a story built with
//! `SpecialCharacters.DISCRETIONARY_HYPHEN` exports `Donau\u{ad}dampf`).
//! Each case is one narrow frame holding one paragraph, so where a line
//! ends is the whole answer: hyphenation on and off, the paragraph's
//! hyphenation limits, its ladder limit, its zone, and words with no soft
//! hyphen as the control.
//!
//! The line grid is the `forced-line-break` fixture's: Inter 10/12, a
//! `LeadingOffset` first baseline and zero insets.

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

const SAMPLE: &str = "soft-hyphens";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
/// The column the cases are set in: narrow enough that the long words
/// must break somewhere, and wide enough that "not the last word" has a
/// last word to protect (InDesign, swept 84..98 pt on this file).
pub const FRAME_W: f32 = 92.0;
const FRAME_H: f32 = 100.0;

/// U+00AD — the discretionary hyphen.
pub const SHY: char = '\u{00ad}';

/// The Word measurement plugin-doc brought (`3f6e8e9`): a 34-letter
/// compound with a soft hyphen between each of its parts.
const DONAU: &str = "The Donau~dampf~schiff~fahrts~gesell~schaft sails today.";
/// Long words, each guarded by a LEADING soft hyphen: justified, the
/// dictionary would break them (InDesign: "Typesetting de- / mands
/// extraordi- / nary") but the guard keeps every one whole.
const GUARDED: &str =
    "~Typesetting ~demands ~extraordinary ~concentration and ~considerable ~patience";
/// Short soft-hyphenated words, two letters before the first break: the
/// breaks a paragraph's hyphenation limits would forbid.
const SHORT: &str = "We ex~amine ab~solutely un~usual ap~proaches to in~formation";
/// `SHORT` capitalised.
const CAPS: &str = "Ex~amine Ab~solutely Un~usual Ap~proaches to In~formation";
/// At 94 pt the single-line composer ends two consecutive lines on a soft
/// hyphen ("ap-", "in-"), which a ladder limit of 1 forbids.
const LADDER: &str =
    "Our in~ventive col~leagues de~velop un~usual ap~proaches to in~formation de~sign";
/// One compound longer than three lines: every line must end on a soft
/// hyphen, so a ladder limit has no way out.
const COMPOUND: &str = "Donau~dampf~schiff~fahrts~gesell~schafts~kapitaens~muetzen~ab~zeichen";

/// One case: a frame, one paragraph. `text` uses `~` for U+00AD.
pub struct Case {
    pub name: &'static str,
    pub text: &'static str,
    /// Paragraph attributes on top of `Hyphenation`.
    pub attrs: Vec<(&'static str, &'static str)>,
    pub hyphenation: bool,
    /// The frame's width, pt.
    pub width: f32,
}

const fn case(name: &'static str, text: &'static str, hyphenation: bool) -> Case {
    Case {
        name,
        text,
        attrs: Vec::new(),
        hyphenation,
        width: FRAME_W,
    }
}

const JUSTIFIED: (&str, &str) = ("Justification", "LeftJustified");
const SINGLE: (&str, &str) = ("Composer", "HL Single");

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    let with = |c: Case, attrs: &[(&'static str, &'static str)]| Case {
        attrs: attrs.to_vec(),
        ..c
    };
    let ladder = ("HyphenateLadderLimit", "1");
    // The limit cases are set by the Single-line Composer with no zone at
    // 96 pt, where every limit changes InDesign's lines (swept 84..100 pt):
    // what they pin is the limits, not a composer's taste.
    let limited = |name, text, hyphenation, extra: &[(&'static str, &'static str)]| {
        let mut attrs = vec![SINGLE, ("HyphenationZone", "0")];
        attrs.extend_from_slice(extra);
        Case {
            width: 96.0,
            ..with(case(name, text, hyphenation), &attrs)
        }
    };
    vec![
        case("off: soft hyphens", DONAU, false),
        case("on: soft hyphens", DONAU, true),
        with(case("off: single-line", DONAU, false), &[SINGLE]),
        with(case("on: justified, guarded", GUARDED, true), &[JUSTIFIED]),
        limited("off: short words", SHORT, false, &[]),
        limited("on: short words", SHORT, true, &[]),
        limited(
            "off: after/before 6",
            SHORT,
            false,
            &[("HyphenateAfterFirst", "6"), ("HyphenateBeforeLast", "6")],
        ),
        limited(
            "off: words of 25+",
            SHORT,
            false,
            &[("HyphenateWordsLongerThan", "25")],
        ),
        limited(
            "off: not the last word",
            SHORT,
            false,
            &[("HyphenateLastWord", "false")],
        ),
        limited(
            "off: no capitals",
            CAPS,
            false,
            &[("HyphenateCapitalizedWords", "false")],
        ),
        limited(
            "on: no capitals",
            CAPS,
            true,
            &[("HyphenateCapitalizedWords", "false")],
        ),
        Case {
            width: 94.0,
            ..with(case("off: single, 94 pt", LADDER, false), &[SINGLE])
        },
        Case {
            width: 94.0,
            ..with(
                case("off: single, ladder 1", LADDER, false),
                &[SINGLE, ladder],
            )
        },
        with(
            case("off: single, zone 36", SHORT, false),
            &[SINGLE, ("HyphenationZone", "36")],
        ),
        with(
            case("off: ladder 1, compound", COMPOUND, false),
            &[SINGLE, ladder],
        ),
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// Top-left of case `i`'s body frame, page-local pt.
pub fn frame_origin(i: u32) -> (f32, f32) {
    let col = i % 3;
    let row = i / 3;
    (72.0 + col as f32 * 160.0, 96.0 + row as f32 * 140.0)
}

/// The case's text with `~` spelt as U+00AD.
pub fn text_of(case: &Case) -> String {
    case.text
        .chars()
        .map(|c| if c == '~' { SHY } else { c })
        .collect()
}

fn paragraph(case: &Case) -> Paragraph {
    let mut attrs = vec![(
        "Hyphenation",
        if case.hyphenation { "true" } else { "false" },
    )];
    attrs.extend(case.attrs.iter().copied());
    Paragraph {
        extra_paragraph_attrs: attrs,
        leading: Some(LEADING),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text_of(case),
            point_size: Some(POINT_SIZE),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

fn frame(id: String, story: String, w: f32, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        // No stroke: a stroke insets the text (`stroke-inset`), and these
        // cases are tuned to a 90 pt measure to the point.
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
            first_baseline_offset: Some("LeadingOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items = Vec::new();

    for (i, case) in cases().into_iter().enumerate() {
        let seq = i as u32;
        let (x, y) = frame_origin(seq);
        let label_story = self_id(SAMPLE, "LabelStory", seq);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph {
                    extra_paragraph_attrs: vec![("Hyphenation", "false")],
                    ..Paragraph::plain(case.name)
                }],
            }),
        ));
        story_refs.push(label_story.clone());
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label_story,
            150.0,
            20.0,
            (x, y - 24.0),
            false,
        ));

        let body = body_story_id(seq);
        stories.push((
            body.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body.clone(),
                paragraphs: vec![paragraph(&case)],
            }),
        ));
        story_refs.push(body.clone());
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            case.width,
            FRAME_H,
            (x, y),
            true,
        ));
    }

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
            stories: story_refs,
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
