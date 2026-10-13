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

//! `kerning.idml` — `KerningMethod`, asked of InDesign (ADR 120).
//!
//! InDesign writes the value with its `$ID/` prefix (`$ID/Metrics`,
//! `$ID/Optical`, `$ID/None`); hand-written and older files spell it
//! bare. Every row sets the same specimen — kerning pairs and digit runs
//! in Inter 20 pt, a face whose GPOS kerns these pairs — under one
//! spelling, from one seat:
//!
//! * rows 1–7 — on the character run: no value, then Metrics, Optical
//!   and None, each bare and with `$ID/`;
//! * rows 8–13 — from the paragraph style (the run names none);
//! * rows 14–19 — from a character style applied to the run;
//! * rows 20–25 — `$ID/Metrics`, `$ID/Optical`, `$ID/None` on the run
//!   under `Tracking` +50 and −25 (row 1–7 are tracking 0).
//!
//! The Optical rows are also the calibration set for an optical-kerning
//! pass: InDesign's advances for them are recorded beside the PDF
//! (`kerning.optical-advances.json`).

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

const SAMPLE: &str = "kerning";
const PAGE_W_PT: f32 = 612.0; // US Letter
const PAGE_H_PT: f32 = 792.0;
/// The specimen's face. Not Open Sans: the corpus build of it kerns none
/// of these pairs, in InDesign or in the engine, so Metrics and None
/// would set alike.
const SPECIMEN_FONT: &str = "Inter";
const LABEL_FONT: &str = "Open Sans";
pub const SPECIMEN: &str = "AV To Wa Ty LT 1111 7171";
pub const SPECIMEN_SIZE: f32 = 20.0;
const LABEL_SIZE: f32 = 7.0;
/// Top of row 1 and the distance between rows, page pt.
pub const ROW_TOP: f32 = 60.0;
pub const ROW_PITCH: f32 = 28.0;
/// Left edge of the specimen frames, page pt.
pub const SPECIMEN_X: f32 = 210.0;
const SPECIMEN_W: f32 = 366.0;

/// Where a row's `KerningMethod` is set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seat {
    Run,
    ParagraphStyle,
    CharacterStyle,
}

impl Seat {
    fn label(self) -> &'static str {
        match self {
            Seat::Run => "run",
            Seat::ParagraphStyle => "paragraph style",
            Seat::CharacterStyle => "character style",
        }
    }
}

pub struct Case {
    pub seat: Seat,
    /// The attribute value as written; `None` writes no attribute.
    pub method: Option<&'static str>,
    /// `Tracking` (1/1000 em) on the run; `None` writes none.
    pub tracking: Option<f32>,
}

/// The six spellings: three methods, bare and `$ID/`.
pub const SPELLINGS: [&str; 6] = [
    "Metrics",
    "$ID/Metrics",
    "Optical",
    "$ID/Optical",
    "None",
    "$ID/None",
];

/// The cases, in row order.
pub fn cases() -> Vec<Case> {
    let mut out = vec![Case {
        seat: Seat::Run,
        method: None,
        tracking: None,
    }];
    for seat in [Seat::Run, Seat::ParagraphStyle, Seat::CharacterStyle] {
        out.extend(SPELLINGS.iter().map(|m| Case {
            seat,
            method: Some(m),
            tracking: None,
        }));
    }
    for tracking in [50.0, -25.0] {
        for method in ["$ID/Metrics", "$ID/Optical", "$ID/None"] {
            out.push(Case {
                seat: Seat::Run,
                method: Some(method),
                tracking: Some(tracking),
            });
        }
    }
    out
}

/// The label printed beside row `i` (0-based).
pub fn case_label(i: usize, c: &Case) -> String {
    format!(
        "{} · {} · {} · tracking {}",
        i + 1,
        c.seat.label(),
        c.method.unwrap_or("(none)"),
        c.tracking.map_or("0".to_string(), |t| format!("{t:+}")),
    )
}

/// Each spelling with the `Self` of the paragraph style and of the
/// character style that carry it.
const STYLE_IDS: [(&str, &str, &str); 6] = [
    (
        "Metrics",
        "ParagraphStyle/Kern Metrics",
        "CharacterStyle/Kern Metrics",
    ),
    (
        "$ID/Metrics",
        "ParagraphStyle/Kern ID-Metrics",
        "CharacterStyle/Kern ID-Metrics",
    ),
    (
        "Optical",
        "ParagraphStyle/Kern Optical",
        "CharacterStyle/Kern Optical",
    ),
    (
        "$ID/Optical",
        "ParagraphStyle/Kern ID-Optical",
        "CharacterStyle/Kern ID-Optical",
    ),
    (
        "None",
        "ParagraphStyle/Kern None",
        "CharacterStyle/Kern None",
    ),
    (
        "$ID/None",
        "ParagraphStyle/Kern ID-None",
        "CharacterStyle/Kern ID-None",
    ),
];

fn style_ids(method: &str) -> (&'static str, &'static str) {
    let (_, p, c) = STYLE_IDS
        .iter()
        .find(|(m, _, _)| *m == method)
        .expect("every spelling has its styles");
    (p, c)
}

fn styles() -> Vec<u8> {
    let mut paragraph = String::new();
    let mut character = String::new();
    for (m, p, c) in STYLE_IDS {
        paragraph.push_str(&format!(
            "<ParagraphStyle Self=\"{p}\" Name=\"{}\" KerningMethod=\"{m}\">\
<Properties><BasedOn type=\"object\">ParagraphStyle/$ID/[No paragraph style]</BasedOn>\
</Properties></ParagraphStyle>",
            &p["ParagraphStyle/".len()..]
        ));
        character.push_str(&format!(
            "<CharacterStyle Self=\"{c}\" Name=\"{}\" KerningMethod=\"{m}\"/>",
            &c["CharacterStyle/".len()..]
        ));
    }
    styles_xml_with_raw(&format!(
        "<RootCharacterStyleGroup>{character}</RootCharacterStyleGroup>\
<RootParagraphStyleGroup>{paragraph}</RootParagraphStyleGroup>"
    ))
}

fn run(
    text: String,
    font: &'static str,
    size: f32,
    extra: Vec<(&'static str, &'static str)>,
) -> Run {
    Run {
        extra_char_attrs: extra,
        text,
        point_size: Some(size),
        fill_color: Some("Color/Black".to_string()),
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: Some(font),
        anchored_frame: None,
    }
}

fn specimen_paragraph(c: &Case) -> Paragraph {
    let mut para_attrs = vec![("Hyphenation", "false")];
    let mut char_attrs = Vec::new();
    match (c.seat, c.method) {
        (Seat::Run, Some(m)) => char_attrs.push(("KerningMethod", m)),
        (Seat::ParagraphStyle, Some(m)) => {
            para_attrs.push(("AppliedParagraphStyle", style_ids(m).0));
        }
        (Seat::CharacterStyle, Some(m)) => {
            char_attrs.push(("AppliedCharacterStyle", style_ids(m).1));
        }
        (_, None) => {}
    }
    let mut r = run(
        SPECIMEN.to_string(),
        SPECIMEN_FONT,
        SPECIMEN_SIZE,
        char_attrs,
    );
    r.tracking = c.tracking;
    Paragraph {
        extra_paragraph_attrs: para_attrs,
        leading: Some(24.0),
        justification: Some("LeftAlign"),
        runs: vec![r],
        ..Paragraph::plain("")
    }
}

fn label_paragraph(text: String) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(LABEL_SIZE * 1.2),
        justification: Some("LeftAlign"),
        runs: vec![run(text, LABEL_FONT, LABEL_SIZE, Vec::new())],
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

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut items = Vec::new();
    let mut story = |id: String, p: Paragraph| {
        stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs: vec![p],
            }),
        ));
        id
    };
    let title = story(
        self_id(SAMPLE, "TitleStory", 0),
        label_paragraph(format!(
            "kerning · KerningMethod × spelling × seat × tracking · {SPECIMEN_FONT} {SPECIMEN_SIZE} pt"
        )),
    );
    items.push(frame(
        self_id(SAMPLE, "TitleFrame", 0),
        title,
        (540.0, 12.0),
        (36.0, 30.0),
    ));
    for (i, c) in cases().iter().enumerate() {
        let seq = i as u32;
        let y = ROW_TOP + i as f32 * ROW_PITCH;
        let label = story(
            self_id(SAMPLE, "LabelStory", seq),
            label_paragraph(case_label(i, c)),
        );
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label,
            (170.0, 12.0),
            (36.0, y + 10.0),
        ));
        let body = story(self_id(SAMPLE, "Story", seq), specimen_paragraph(c));
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            (SPECIMEN_W, 26.0),
            (SPECIMEN_X, y),
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
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader every seat carries its spelling:
    /// the run's attribute, and each style's.
    #[test]
    fn every_spelling_reads_back_from_its_seat() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        assert_eq!(cases().len(), 25);
        let on_runs: Vec<_> = doc
            .stories
            .iter()
            .flat_map(|s| s.story.paragraphs.iter())
            .flat_map(|p| p.runs.iter())
            .filter_map(|r| r.kerning_method.as_deref())
            .collect();
        for m in SPELLINGS {
            assert!(on_runs.contains(&m), "{m} on a run");
        }
        let text = String::from_utf8(styles()).expect("utf-8");
        for m in SPELLINGS {
            assert!(text.contains(&format!("KerningMethod=\"{m}\"")), "{m}");
        }
    }
}
