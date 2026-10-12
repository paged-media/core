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

//! `tints.idml` — how a tint of a CMYK colour prints, asked of
//! InDesign (ADR 120).
//!
//! A tint reaches a page two ways: as the object's own `FillTint` on a
//! full-strength swatch, and as a named `<Tint BaseColor TintValue>`
//! swatch applied at 100 %. This fixture puts the two side by side for
//! every value, so the reference shows whether they agree and the
//! renderer can be held to the same.
//!
//! Page 1 — cases 1–54: nine CMYK colours (rows: process C, M, Y, the
//! `[Black]` swatch, a rich black and four brand colours) × tint 100,
//! 80, 60, 40, 20 and 5 % (column pairs). In each pair the LEFT square
//! is the base swatch at `FillTint`, the RIGHT square the `<Tint>`
//! swatch of the same value.
//!
//! Page 2:
//! * 55 — an 8 pt teal stroke at `StrokeTint="40"` beside one painted
//!   with the teal 40 % `<Tint>` swatch;
//! * 56 — 48 pt text: teal at `FillTint="40"` then the teal 40 %
//!   `<Tint>` swatch;
//! * 57 — a gradient whose first stop is the teal 40 % `<Tint>` swatch,
//!   to orange;
//! * 58 — a spot colour defined in CMYK (alternate the same CMYK) at
//!   `FillTint` 100, 60 and 20, then its 60 % and 20 % `<Tint>`
//!   swatches;
//! * 59 — the teal 50 % `<Tint>` swatch at `FillTint="50"` (a tint of a
//!   tint) beside the teal 25 % `<Tint>` swatch.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml_rich_full, preferences_xml, styles_xml,
        ExtraGradient, GradientStop, RichColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "tints";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const LABEL_FONT: &str = "Open Sans";
const LABEL_SIZE: f32 = 7.0;

/// The tint values, one column pair each.
pub const TINTS: [f32; 6] = [100.0, 80.0, 60.0, 40.0, 20.0, 5.0];
/// Page-1 square edge, pt.
pub const SQUARE: f32 = 28.0;
/// Page-1 left edge of each column pair's FillTint square; the
/// `<Tint>` swatch square sits `SQUARE + 2` to its right.
pub const PAIR_LEFT: [f32; 6] = [110.0, 182.0, 254.0, 326.0, 398.0, 470.0];
/// Page-1 top of row `i`.
pub fn row_top(i: usize) -> f32 {
    80.0 + i as f32 * 44.0
}

/// One process colour: `(self id, name, CMYK)`.
pub const COLORS: [(&str, &str, &str); 9] = [
    ("Color/ProcessCyan", "Process Cyan", "100 0 0 0"),
    ("Color/ProcessMagenta", "Process Magenta", "0 100 0 0"),
    ("Color/ProcessYellow", "Process Yellow", "0 0 100 0"),
    ("Color/Black", "Black", "0 0 0 100"),
    ("Color/RichBlack", "Rich Black", "60 40 40 100"),
    ("Color/BrandTeal", "Brand Teal", "85 10 40 0"),
    ("Color/BrandOrange", "Brand Orange", "0 60 95 0"),
    ("Color/BrandPlum", "Brand Plum", "55 90 10 5"),
    ("Color/BrandOlive", "Brand Olive", "35 15 85 20"),
];

pub const SPOT: &str = "Color/SpotRed";
const SPOT_CMYK: &str = "0 90 85 0";
pub const TEAL: &str = "Color/BrandTeal";
pub const ORANGE: &str = "Color/BrandOrange";

/// The `<Tint>` swatch id for `base` at `t` %.
pub fn tint_id(base: &str, t: f32) -> String {
    format!("{base}T{}", t as u32)
}

fn rich(self_id: &str, name: &str, cmyk: &str) -> RichColor {
    RichColor {
        self_id: self_id.to_string(),
        name: name.to_string(),
        model: "Process",
        space: "CMYK",
        value: cmyk.to_string(),
        alternate_space: None,
        alternate_value: None,
        tint: None,
        base_color: None,
    }
}

fn tint_swatch(base: &RichColor, t: f32) -> RichColor {
    RichColor {
        self_id: tint_id(&base.self_id, t),
        name: format!("{} {}%", base.name, t as u32),
        model: base.model,
        space: base.space,
        value: base.value.clone(),
        alternate_space: base.alternate_space,
        alternate_value: base.alternate_value.clone(),
        tint: Some(t),
        base_color: Some(base.self_id.clone()),
    }
}

fn colors() -> Vec<RichColor> {
    let mut bases: Vec<RichColor> = COLORS
        .iter()
        // `[Black]` is one of the two built-ins the writer always emits.
        .filter(|(id, _, _)| *id != "Color/Black")
        .map(|(id, name, cmyk)| rich(id, name, cmyk))
        .collect();
    bases.push(RichColor {
        model: "Spot",
        alternate_space: Some("CMYK"),
        alternate_value: Some(SPOT_CMYK.to_string()),
        ..rich(SPOT, "Spot Red", SPOT_CMYK)
    });
    let black = rich("Color/Black", "Black", "0 0 0 100");
    let mut out = Vec::new();
    for (id, _, _) in COLORS {
        let base = if id == "Color/Black" {
            &black
        } else {
            bases.iter().find(|b| b.self_id == id).expect("base")
        };
        for t in TINTS {
            out.push(tint_swatch(base, t));
        }
    }
    let spot = bases.iter().find(|b| b.self_id == SPOT).expect("spot");
    out.push(tint_swatch(spot, 60.0));
    out.push(tint_swatch(spot, 20.0));
    let teal = bases.iter().find(|b| b.self_id == TEAL).expect("teal");
    out.push(tint_swatch(teal, 50.0));
    out.push(tint_swatch(teal, 25.0));
    // Bases first: a `<Tint>` names its base by reference.
    bases.extend(out);
    bases
}

fn gradients() -> Vec<ExtraGradient> {
    vec![ExtraGradient {
        self_id: "Gradient/TealTint40ToOrange".to_string(),
        name: "Teal 40% to Orange".to_string(),
        kind: "Linear",
        stops: vec![
            GradientStop {
                stop_color: tint_id(TEAL, 40.0),
                location_pct: 0.0,
                midpoint_pct: None,
            },
            GradientStop {
                stop_color: ORANGE.to_string(),
                location_pct: 100.0,
                midpoint_pct: None,
            },
        ],
    }]
}

fn label_run(text: &str) -> Run {
    Run {
        point_size: Some(LABEL_SIZE),
        applied_font: Some(LABEL_FONT),
        fill_color: Some("Color/Black".to_string()),
        ..Run::with_char_attrs(text, Vec::new())
    }
}

struct Page {
    items: Vec<PageItem>,
    stories: Vec<(String, Vec<u8>)>,
    seq: u32,
}

impl Page {
    fn new(seq: u32) -> Self {
        Self {
            items: Vec::new(),
            stories: Vec::new(),
            seq,
        }
    }

    fn next(&mut self, kind: &str) -> String {
        self.seq += 1;
        self_id(SAMPLE, kind, self.seq)
    }

    fn story(&mut self, paragraphs: Vec<Paragraph>) -> String {
        let id = self.next("Story");
        self.stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs,
            }),
        ));
        id
    }

    fn text_frame(&mut self, runs: Vec<Run>, leading: f32, x: f32, y: f32, w: f32, h: f32) {
        let story = self.story(vec![Paragraph {
            runs,
            leading: Some(leading),
            ..Paragraph::plain("")
        }]);
        let id = self.next("TextFrame");
        let mut r = Rect::filled(id, w, h, translate(x, y)).with_parent_story(story);
        r.fill_color = None;
        self.items.push(r.into());
    }

    fn label(&mut self, text: &str, x: f32, y: f32, w: f32) {
        self.text_frame(vec![label_run(text)], LABEL_SIZE * 1.2, x, y, w, 12.0);
    }

    /// A filled square/rectangle, optionally at an object `FillTint`.
    fn fill(&mut self, color: &str, fill_tint: Option<f32>, x: f32, y: f32, w: f32, h: f32) {
        let id = self.next("Rectangle");
        let mut r = Rect::filled(id, w, h, translate(x, y)).with_fill(color);
        if let Some(t) = fill_tint {
            r = r.with_attr("FillTint", crate::xml::format_f32(t));
        }
        self.items.push(r.into());
    }
}

fn page_one() -> Page {
    let mut p = Page::new(0);
    p.label(
        "tints · page 1 · in each pair: LEFT = swatch at FillTint, RIGHT = <Tint> swatch of the same value",
        36.0,
        30.0,
        520.0,
    );
    for (col, t) in TINTS.iter().enumerate() {
        p.label(&format!("{}%", *t as u32), PAIR_LEFT[col], 62.0, 50.0);
    }
    for (row, (id, name, cmyk)) in COLORS.iter().enumerate() {
        let top = row_top(row);
        let first = row * 6 + 1;
        p.label(
            &format!("{}–{} · {name} {cmyk}", first, first + 5),
            36.0,
            top + 8.0,
            72.0,
        );
        for (col, t) in TINTS.iter().enumerate() {
            let x = PAIR_LEFT[col];
            p.fill(id, Some(*t), x, top, SQUARE, SQUARE);
            p.fill(
                &tint_id(id, *t),
                None,
                x + SQUARE + 2.0,
                top,
                SQUARE,
                SQUARE,
            );
        }
    }
    p
}

fn page_two() -> Page {
    let mut p = Page::new(1000);
    p.label(
        "tints · page 2 · strokes, text, a gradient stop, a spot, a tint of a tint",
        36.0,
        30.0,
        520.0,
    );

    // 55 — stroke tint.
    p.label(
        "55 · 8 pt stroke: teal at StrokeTint 40 | teal 40% <Tint> swatch",
        36.0,
        60.0,
        400.0,
    );
    for (i, (color, tint)) in [(TEAL.to_string(), Some(40.0)), (tint_id(TEAL, 40.0), None)]
        .into_iter()
        .enumerate()
    {
        let id = p.next("Rectangle");
        let mut r = Rect::filled(id, 120.0, 60.0, translate(48.0 + i as f32 * 160.0, 84.0))
            .with_stroke(color, 8.0);
        r.fill_color = None;
        if let Some(t) = tint {
            r = r.with_attr("StrokeTint", crate::xml::format_f32(t));
        }
        p.items.push(r.into());
    }

    // 56 — text tint.
    p.label(
        "56 · 48 pt text: teal at FillTint 40 | teal 40% <Tint> swatch",
        36.0,
        170.0,
        400.0,
    );
    let big = |text: &str, color: String, attrs: Vec<(&'static str, &'static str)>| Run {
        point_size: Some(48.0),
        applied_font: Some(LABEL_FONT),
        fill_color: Some(color),
        ..Run::with_char_attrs(text, attrs)
    };
    p.text_frame(
        vec![
            big("HHHH ", TEAL.to_string(), vec![("FillTint", "40")]),
            big("HHHH", tint_id(TEAL, 40.0), Vec::new()),
        ],
        58.0,
        36.0,
        184.0,
        420.0,
        64.0,
    );

    // 57 — a gradient stop on a tint swatch.
    p.label(
        "57 · gradient: teal 40% <Tint> swatch → brand orange",
        36.0,
        270.0,
        400.0,
    );
    p.fill(
        "Gradient/TealTint40ToOrange",
        None,
        36.0,
        284.0,
        300.0,
        50.0,
    );

    // 58 — a spot colour defined in CMYK.
    p.label(
        "58 · spot red (CMYK 0 90 85 0): FillTint 100, 60, 20 | <Tint> 60%, 20%",
        36.0,
        360.0,
        400.0,
    );
    for (i, t) in [100.0, 60.0, 20.0].iter().enumerate() {
        p.fill(SPOT, Some(*t), 36.0 + i as f32 * 50.0, 374.0, 40.0, 40.0);
    }
    for (i, t) in [60.0, 20.0].iter().enumerate() {
        p.fill(
            &tint_id(SPOT, *t),
            None,
            236.0 + i as f32 * 50.0,
            374.0,
            40.0,
            40.0,
        );
    }

    // 59 — a tint of a tint.
    p.label(
        "59 · teal 50% <Tint> swatch at FillTint 50 | teal 25% <Tint> swatch",
        36.0,
        440.0,
        400.0,
    );
    p.fill(&tint_id(TEAL, 50.0), Some(50.0), 36.0, 454.0, 60.0, 40.0);
    p.fill(&tint_id(TEAL, 25.0), None, 106.0, 454.0, 60.0, 40.0);
    p
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let pages = [page_one(), page_two()];
    let mut master_spreads = Vec::new();
    let mut master_refs = Vec::new();
    let mut spreads = Vec::new();
    let mut spread_refs = Vec::new();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    for (i, page) in pages.into_iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", seq),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());
        let spread_id = self_id(SAMPLE, "Spread", seq);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: format!("tints {}", i + 1),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: page.items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        ));
        spread_refs.push(spread_id);
        for (id, xml) in page.stories {
            story_refs.push(id.clone());
            stories.push((id, xml));
        }
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: master_refs,
            spreads: spread_refs,
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_rich_full(&colors(), &gradients(), &[], &[]),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader every `<Tint>` swatch resolves to
    /// its base's inks scaled by its value.
    #[test]
    fn every_tint_swatch_reads_back_scaled() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        for (id, _, cmyk) in COLORS {
            let base: Vec<f32> = cmyk.split(' ').map(|v| v.parse().unwrap()).collect();
            for t in TINTS {
                let e = doc
                    .palette
                    .resolve(&tint_id(id, t))
                    .unwrap_or_else(|| panic!("{id} at {t}"));
                let got = e.effective_cmyk().expect("cmyk");
                for ch in 0..4 {
                    assert!(
                        (got[ch] - base[ch] * t / 100.0).abs() < 1e-3,
                        "{id} {t}: {got:?}"
                    );
                }
            }
        }
    }
}
