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

//! `rect-shadows.idml` — what casts a rectangle's or a text frame's
//! drop shadow, asked of InDesign (ADR 130).
//!
//! `line-shadows` showed InDesign casts an oval's and a polygon's
//! shadow from what the shape paints: the stroke band when there is no
//! fill, the outline pushed out by the stroke's outer part when there
//! is both. This page asks the same of the rectangular kinds. Every
//! item carries the same shadow — offset 6 / 6 pt, `Size` 4 pt, black
//! at 75 % — and every frame is 120 × 80 pt:
//!
//! * row 1 — stroke-only rectangles: 6 pt and 1 pt centred, 6 pt
//!   outside;
//! * row 2 — filled (grey 40 %), 6 pt stroked rectangles: centre,
//!   inside, outside;
//! * row 3 — a filled, stroked rounded rectangle (radius 16), a
//!   stroke-only rounded rectangle, a filled rectangle with no stroke;
//! * row 4 — text frames holding "Hi" in Inter 48 pt: stroke-only,
//!   filled and stroked, and neither filled nor stroked.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{DropShadow, PageItem, Rect, TextFramePref},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "rect-shadows";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
/// Left edge of each of the three columns.
pub const COLUMNS: [f32; 3] = [70.0, 240.0, 410.0];
/// Top edge of each of the four rows.
pub const ROWS: [f32; 4] = [80.0, 250.0, 420.0, 590.0];
pub const FRAME_W: f32 = 120.0;
pub const FRAME_H: f32 = 80.0;
const TEXT_FONT: &str = "Inter";
const TEXT_SIZE: f32 = 48.0;

/// One case: a frame at (column, row).
pub struct Case {
    pub name: &'static str,
    pub column: usize,
    pub row: usize,
    pub fill: bool,
    /// `StrokeWeight`; `0` writes no stroke.
    pub weight: f32,
    pub alignment: Option<&'static str>,
    /// `CornerOption="RoundedCorner"` at this radius.
    pub radius: Option<f32>,
    /// A text frame holding "Hi".
    pub text: bool,
}

const fn case(name: &'static str, column: usize, row: usize, fill: bool, weight: f32) -> Case {
    Case {
        name,
        column,
        row,
        fill,
        weight,
        alignment: None,
        radius: None,
        text: false,
    }
}

const INSIDE: Option<&str> = Some("InsideAlignment");
const OUTSIDE: Option<&str> = Some("OutsideAlignment");

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    vec![
        case("stroke-only 6 centre", 0, 0, false, 6.0),
        case("stroke-only 1 centre", 1, 0, false, 1.0),
        Case {
            alignment: OUTSIDE,
            ..case("stroke-only 6 outside", 2, 0, false, 6.0)
        },
        case("filled, 6 centre", 0, 1, true, 6.0),
        Case {
            alignment: INSIDE,
            ..case("filled, 6 inside", 1, 1, true, 6.0)
        },
        Case {
            alignment: OUTSIDE,
            ..case("filled, 6 outside", 2, 1, true, 6.0)
        },
        Case {
            radius: Some(16.0),
            ..case("rounded 16, filled, 6 centre", 0, 2, true, 6.0)
        },
        Case {
            radius: Some(16.0),
            ..case("rounded 16, stroke-only 6 centre", 1, 2, false, 6.0)
        },
        case("filled, no stroke", 2, 2, true, 0.0),
        Case {
            text: true,
            ..case("text, stroke-only 6 centre", 0, 3, false, 6.0)
        },
        Case {
            text: true,
            ..case("text, filled, 6 centre", 1, 3, true, 6.0)
        },
        Case {
            text: true,
            ..case("text, no fill, no stroke", 2, 3, false, 0.0)
        },
    ]
}

/// Top-left of a case's frame, page pt.
pub fn origin(c: &Case) -> (f32, f32) {
    (COLUMNS[c.column], ROWS[c.row])
}

fn shadow() -> DropShadow {
    DropShadow {
        mode: "Drop",
        x_offset: Some(6.0),
        y_offset: Some(6.0),
        size: Some(4.0),
        opacity_pct: Some(75.0),
        effect_color: Some("Color/Black".to_string()),
    }
}

fn frame(i: u32, c: &Case, story: Option<String>) -> PageItem {
    let mut extra_attrs = Vec::new();
    if let Some(a) = c.alignment {
        extra_attrs.push(("StrokeAlignment".to_string(), a.to_string()));
    }
    if let Some(r) = c.radius {
        // InDesign reads the per-corner spelling (see `stroke-inset`).
        extra_attrs.push(("CornerOption".to_string(), "RoundedCorner".to_string()));
        extra_attrs.push(("CornerRadius".to_string(), r.to_string()));
        for corner in ["TopLeft", "TopRight", "BottomLeft", "BottomRight"] {
            extra_attrs.push((format!("{corner}CornerOption"), "RoundedCorner".to_string()));
            extra_attrs.push((format!("{corner}CornerRadius"), r.to_string()));
        }
    }
    let at = origin(c);
    let text = story.is_some();
    Rect {
        self_id: self_id(SAMPLE, if text { "TextFrame" } else { "Rectangle" }, i),
        width_pt: FRAME_W,
        height_pt: FRAME_H,
        item_transform: translate(at.0, at.1),
        fill_color: c.fill.then(|| "Color/Grey40".to_string()),
        stroke_color: (c.weight > 0.0).then(|| "Color/Black".to_string()),
        stroke_weight_pt: Some(c.weight),
        parent_story: story,
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs,
        blending: None,
        drop_shadow: Some(shadow()),
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: text.then(|| TextFramePref {
            inset_spacing: Some([0.0; 4]),
            vertical_justification: Some("CenterAlign"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

fn text_paragraph() -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(TEXT_SIZE * 1.2),
        justification: Some("CenterAlign"),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: "Hi".to_string(),
            point_size: Some(TEXT_SIZE),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(TEXT_FONT),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items = Vec::new();
    for (i, c) in cases().iter().enumerate() {
        let story = c.text.then(|| {
            let id = self_id(SAMPLE, "Story", i as u32);
            stories.push((
                id.clone(),
                write_story(&Story {
                    extra_story_attrs: Vec::new(),
                    self_id: id.clone(),
                    paragraphs: vec![text_paragraph()],
                }),
            ));
            story_refs.push(id.clone());
            id
        });
        items.push(frame(i as u32, c, story));
    }

    let master_spreads = vec![(
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: master_page_id,
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    )];
    let spreads = vec![(
        spread_id.clone(),
        write_spread(&Spread {
            self_id: spread_id.clone(),
            page_self_id: page_id,
            page_name: "rect-shadows · rectangles · text frames".to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: items,
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        }),
    )];

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id],
            spreads: vec![spread_id],
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_with_extras(&[ExtraColor {
            self_id: "Color/Grey40".to_string(),
            name: "Grey 40".to_string(),
            space: "CMYK",
            value: "0 0 0 40".to_string(),
        }]),
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

    /// Through the engine's own reader every frame carries the shadow.
    #[test]
    fn every_frame_reads_back_with_its_shadow() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let s = &doc.spreads[0].spread;
        assert_eq!(s.rectangles.len(), 9);
        assert_eq!(s.text_frames.len(), 3);
        for ds in s
            .rectangles
            .iter()
            .map(|r| &r.drop_shadow)
            .chain(s.text_frames.iter().map(|t| &t.drop_shadow))
        {
            let ds = ds.as_ref().expect("shadow read");
            assert_eq!((ds.x_offset, ds.y_offset, ds.size), (6.0, 6.0, 4.0));
        }
    }
}
