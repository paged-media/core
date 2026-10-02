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

//! `composer.idml` — InDesign's two paragraph composers side by side.
//!
//! Every case is set twice with the same text, measure and spacing: the
//! left frame with the Adobe Paragraph Composer (`Composer="HL
//! Composer"`), the right one with the Adobe Single-line Composer
//! (`Composer="HL Single"`, the value InDesign 20.0.1 writes when a
//! paragraph is set to it). The cases vary what a line breaker trades:
//! ragged and justified, narrow and wide measures, hyphenation on and
//! off, a word longer than the measure, a tight and a loose word-spacing
//! band, and letter spacing.
//!
//! The line grid is the `keeps` fixture's: Inter 10/12, a `LeadingOffset`
//! first baseline and zero insets, so every line of a frame is one 12 pt
//! step below the last and a line's text is all a diff has to read.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "composer";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
/// Body frames per page, as rows of a Paragraph / Single-line pair.
const ROWS_PER_PAGE: u32 = 2;
const ROW_PITCH: f32 = 380.0;
const FRAME_H: f32 = 330.0;
/// A wide frame holds a dozen lines.
const WIDE_FRAME_H: f32 = 160.0;

/// The two composers, in frame order within a pair.
pub const COMPOSERS: [&str; 2] = ["HL Composer", "HL Single"];

/// One case: a paragraph setting, set once per composer.
pub struct Case {
    pub name: &'static str,
    pub measure: f32,
    pub justification: &'static str,
    pub hyphenation: bool,
    /// `(Minimum, Desired, Maximum)WordSpacing`, percent.
    pub word_spacing: Option<(&'static str, &'static str, &'static str)>,
    /// `(Minimum, Desired, Maximum)LetterSpacing`, percent.
    pub letter_spacing: Option<(f32, f32, f32)>,
    /// Further `<ParagraphStyleRange>` attributes.
    pub extra: &'static [(&'static str, &'static str)],
    pub paragraphs: &'static [&'static str],
}

const PROSE_A: &str = "The composer reads the whole paragraph before it sets a single line, \
weighing every place a line could end against every other, and it will happily make an early line \
a little looser if that saves a later one from looking wrong.";
const PROSE_B: &str = "A typesetter working by hand had no such luxury. Each line was filled \
from the left until the next word would not fit, and then the gaps were adjusted until the line \
was exactly as wide as the measure, which is what most word processors still do today.";
const PROSE_C: &str = "Narrow columns are where the difference shows: with only a few words to a \
line there is very little space to share out, so one long word arriving at the wrong moment can \
open a gap that the reader sees from across the room.";
const PROSE_D: &str = "Hyphenation gives the breaker more ways to stop. Unquestionably the \
exceptional characteristics of uncharacteristically comprehensive documentation encourage \
considerable experimentation with typographical conventions.";
const PROSE_E: &str = "Short words in a row, as in a list of it, is, as, to, be, or, at, by, \
on, up, so, no, an, we, he, do, go, me, my, of, if, us, give a line breaker many choices, and \
bigger ones like documentation and responsibilities give it few.";
const PROSE_F: &str = "Internationalization and incomprehensibility arrive together in this \
sentence so that a measure narrower than either word makes the composer break inside them \
even if it does not want to.";

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    let base = Case {
        name: "",
        measure: 130.0,
        justification: "LeftAlign",
        hyphenation: false,
        word_spacing: None,
        letter_spacing: None,
        extra: &[],
        paragraphs: &[PROSE_A, PROSE_B],
    };
    vec![
        Case {
            name: "ragged, no hyphenation",
            ..base
        },
        Case {
            name: "ragged, hyphenation",
            hyphenation: true,
            paragraphs: &[PROSE_C, PROSE_D],
            ..base
        },
        Case {
            name: "justified, no hyphenation",
            justification: "LeftJustified",
            paragraphs: &[PROSE_B, PROSE_C],
            ..base
        },
        Case {
            name: "justified, hyphenation",
            justification: "LeftJustified",
            hyphenation: true,
            paragraphs: &[PROSE_A, PROSE_D],
            ..base
        },
        Case {
            name: "justified, spaces never shrink (100/100/200)",
            justification: "LeftJustified",
            word_spacing: Some(("100", "100", "200")),
            paragraphs: &[PROSE_E, PROSE_A],
            ..base
        },
        Case {
            name: "justified, loose band (60/100/150)",
            justification: "LeftJustified",
            word_spacing: Some(("60", "100", "150")),
            paragraphs: &[PROSE_E, PROSE_A],
            ..base
        },
        Case {
            name: "justified, letter spacing (-5/0/10)",
            justification: "LeftJustified",
            letter_spacing: Some((-5.0, 0.0, 10.0)),
            paragraphs: &[PROSE_B, PROSE_C],
            ..base
        },
        Case {
            name: "ragged, shrinkable spaces (70/100/133)",
            word_spacing: Some(("70", "100", "133")),
            paragraphs: &[PROSE_E, PROSE_B],
            ..base
        },
        Case {
            name: "long words, 80 pt, hyphenation",
            measure: 80.0,
            justification: "LeftJustified",
            hyphenation: true,
            paragraphs: &[PROSE_F],
            ..base
        },
        Case {
            name: "ragged, hyphenation zone 0",
            hyphenation: true,
            extra: &[("HyphenationZone", "0")],
            paragraphs: &[PROSE_D, PROSE_C],
            ..base
        },
        Case {
            name: "ragged, desired 80 (80/80/100)",
            word_spacing: Some(("80", "80", "100")),
            paragraphs: &[PROSE_C, PROSE_E],
            ..base
        },
        Case {
            name: "centred, hyphenation",
            justification: "CenterAlign",
            hyphenation: true,
            paragraphs: &[PROSE_A, PROSE_D],
            ..base
        },
        Case {
            name: "justified wide, hyphenation",
            measure: 451.0,
            justification: "LeftJustified",
            hyphenation: true,
            paragraphs: &[PROSE_A, PROSE_B, PROSE_D],
            ..base
        },
        Case {
            name: "ragged wide, hyphenation",
            measure: 451.0,
            hyphenation: true,
            paragraphs: &[PROSE_C, PROSE_E, PROSE_F],
            ..base
        },
    ]
}

/// A wide case's two frames stack instead of sitting side by side.
fn is_wide(case: &Case) -> bool {
    case.measure > 220.0
}

/// The body story of case `case`, composer `composer` (0 = Paragraph,
/// 1 = Single-line).
pub fn body_story_id(case: u32, composer: u32) -> String {
    self_id(SAMPLE, "BodyStory", case * 2 + composer)
}

/// Where case `i`'s two frames sit: (page index, [(x, y); 2]) in
/// page-local pt. Narrow cases pair up on a row; a wide case fills a
/// row with each composer, so it takes two.
pub fn placement() -> Vec<(u32, [(f32, f32); 2])> {
    let mut out = Vec::new();
    let mut slot = 0u32;
    for case in cases() {
        let row = |s: u32| {
            (
                s / ROWS_PER_PAGE,
                60.0 + (s % ROWS_PER_PAGE) as f32 * ROW_PITCH,
            )
        };
        if is_wide(&case) {
            // Never split a wide pair across pages.
            if slot % ROWS_PER_PAGE == ROWS_PER_PAGE - 1 {
                slot += 1;
            }
            let (p, y0) = row(slot);
            let (_, y1) = row(slot + 1);
            out.push((p, [(72.0, y0), (72.0, y1)]));
            slot += 2;
        } else {
            let (p, y) = row(slot);
            out.push((p, [(72.0, y), (322.0, y)]));
            slot += 1;
        }
    }
    out
}

/// A case that sets word or letter spacing does it through a paragraph
/// style, one per composer, which also names the composer: the importer
/// reads spacing off a style (a `<ParagraphStyleRange>` override of it is
/// not modelled yet), and the style-level `Composer` must cascade.
fn has_style(case: &Case) -> bool {
    case.word_spacing.is_some() || case.letter_spacing.is_some()
}

fn style_id(case: u32, composer: usize) -> &'static str {
    // `extra_paragraph_attrs` holds `&'static str`s; a fixture builds
    // once per process.
    Box::leak(format!("ParagraphStyle/Composer Case {case} {composer}").into_boxed_str())
}

/// The `<ParagraphStyle>`s the spacing cases apply.
fn styles_fragment() -> String {
    let mut out = String::from("<RootParagraphStyleGroup>");
    for (i, case) in cases().iter().enumerate() {
        if !has_style(case) {
            continue;
        }
        for (c, composer) in COMPOSERS.iter().enumerate() {
            let id = style_id(i as u32, c);
            let mut attrs = format!(
                r#"Self="{id}" Name="{}" Composer="{composer}""#,
                id.trim_start_matches("ParagraphStyle/")
            );
            if let Some((min, desired, max)) = case.word_spacing {
                attrs += &format!(
                    r#" MinimumWordSpacing="{min}" DesiredWordSpacing="{desired}" MaximumWordSpacing="{max}""#
                );
            }
            if let Some((min, desired, max)) = case.letter_spacing {
                attrs += &format!(
                    r#" MinimumLetterSpacing="{min}" DesiredLetterSpacing="{desired}" MaximumLetterSpacing="{max}""#
                );
            }
            out += &format!(
                r#"<ParagraphStyle {attrs}><Properties><BasedOn type="string">$ID/[No paragraph style]</BasedOn></Properties></ParagraphStyle>"#
            );
        }
    }
    out + "</RootParagraphStyleGroup>"
}

fn paragraph(case: &Case, index: u32, composer: usize, text: &str) -> Paragraph {
    let mut extra = vec![(
        "Hyphenation",
        if case.hyphenation { "true" } else { "false" },
    )];
    if has_style(case) {
        extra.push(("AppliedParagraphStyle", style_id(index, composer)));
    } else {
        extra.push(("Composer", COMPOSERS[composer]));
    }
    extra.extend_from_slice(case.extra);
    Paragraph {
        extra_paragraph_attrs: extra,
        leading: Some(LEADING),
        justification: Some(case.justification),
        runs: vec![Run {
            // InDesign composes an IDML without a language in its own UI
            // language; this machine's is German, which hyphenates
            // `Hy-phe-na-ti-on`.
            extra_char_attrs: vec![("AppliedLanguage", "$ID/English: USA")],
            text: text.to_string(),
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
    let cases = cases();
    let placement = placement();
    let pages = placement.iter().map(|(p, _)| *p).max().unwrap_or(0) + 1;
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items: Vec<Vec<PageItem>> = (0..pages).map(|_| Vec::new()).collect();

    for (i, (case, (page, at))) in cases.iter().zip(&placement).enumerate() {
        for (c, &(x, y)) in at.iter().enumerate() {
            let seq = (i * 2 + c) as u32;
            let label_story = self_id(SAMPLE, "LabelStory", seq);
            let label = format!(
                "{}. {} — {}",
                i + 1,
                case.name,
                if c == 0 { "paragraph" } else { "single-line" }
            );
            stories.push((
                label_story.clone(),
                write_story(&Story {
                    extra_story_attrs: Vec::new(),
                    self_id: label_story.clone(),
                    paragraphs: vec![Paragraph::plain(label)],
                }),
            ));
            story_refs.push(label_story.clone());
            items[*page as usize].push(frame(
                self_id(SAMPLE, "LabelFrame", seq),
                label_story,
                case.measure.max(220.0),
                20.0,
                (x, y - 22.0),
                false,
            ));

            let body = body_story_id(i as u32, c as u32);
            stories.push((
                body.clone(),
                write_story(&Story {
                    extra_story_attrs: Vec::new(),
                    self_id: body.clone(),
                    paragraphs: case
                        .paragraphs
                        .iter()
                        .map(|t| paragraph(case, i as u32, c, t))
                        .collect(),
                }),
            ));
            story_refs.push(body.clone());
            let h = if is_wide(case) { WIDE_FRAME_H } else { FRAME_H };
            items[*page as usize].push(frame(
                self_id(SAMPLE, "Frame", seq),
                body,
                case.measure,
                h,
                (x, y),
                true,
            ));
        }
    }

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let mut spreads = Vec::new();
    for (p, page_items) in items.into_iter().enumerate() {
        let spread_id = self_id(SAMPLE, "Spread", p as u32);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", p as u32),
                page_name: (p + 1).to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        ));
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: story_refs,
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml_with_raw(&styles_fragment()),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads,
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_body_paragraph_names_its_composer() {
        let sample = build();
        let ids: Vec<String> = (0..cases().len() as u32)
            .flat_map(|i| [body_story_id(i, 0), body_story_id(i, 1)])
            .collect();
        let bodies: Vec<_> = sample
            .stories
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .collect();
        assert_eq!(bodies.len(), cases().len() * 2);
        for (_, xml) in bodies {
            let xml = String::from_utf8_lossy(xml);
            assert!(
                xml.contains(r#"Composer="HL Composer""#)
                    ^ xml.contains(r#"Composer="HL Single""#)
                    ^ xml.contains("ParagraphStyle/Composer Case"),
                "every story names its composer, directly or through its style"
            );
        }
    }

    #[test]
    fn no_frame_leaves_its_page() {
        for (page, at) in placement() {
            let _ = page;
            for (_, y) in at {
                assert!(y + FRAME_H <= PAGE_H_PT - 36.0);
            }
        }
    }
}
