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

//! `line-shadows.idml` — what casts a drop shadow on a shape with no
//! fill, asked of InDesign (ADR 130).
//!
//! A line has no fill, so the engine casts its shadow from the STROKE:
//! the centreline stroked at the line's width, caps and join. This page
//! puts that next to the shapes whose shadow was already measured.
//! Every item carries the same shadow — offset 6 / 6 pt, `Size` 4 pt,
//! black at 75 % — and no text, so the reference measures shadows only:
//!
//! * row 1 — `<GraphicLine>`s: 6 pt round caps, 1 pt butt caps;
//! * row 2 — a 6 pt round-capped open `<Polygon>` (a pen path) and a
//!   6 pt diagonal line with projecting caps;
//! * row 3 — references: a CLOSED stroke-only triangle (6 pt), a filled
//!   triangle with no stroke, and a filled, stroked oval.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{GraphicLine, Oval, PageItem, PathPoint, Polygon, PolygonSubPath},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "line-shadows";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
/// Left edge of each of the three columns.
const COLUMNS: [f32; 3] = [70.0, 240.0, 410.0];

/// The one shadow every item carries, in InDesign's spelling
/// (`TransparencySetting` is a sibling of `Properties`).
const SHADOW: &str = r#"<TransparencySetting><DropShadowSetting Mode="Drop" XOffset="6" YOffset="6" Size="4" Opacity="75" EffectColor="Color/Black"/></TransparencySetting>"#;

fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn line(i: u32, x: f32, y: f32, weight: f32, cap: &str, to: (f32, f32)) -> PageItem {
    GraphicLine {
        self_id: self_id(SAMPLE, "GraphicLine", i),
        item_transform: translate(x, y),
        stroke_color: Some("Color/Black".into()),
        stroke_weight_pt: Some(weight),
        extra_attrs: attrs(&[("EndCap", cap)]),
        points: vec![PathPoint::corner((0.0, 0.0)), PathPoint::corner(to)],
    }
    .into()
}

#[allow(clippy::too_many_arguments)]
fn polygon(
    i: u32,
    x: f32,
    y: f32,
    pts: Vec<(f32, f32)>,
    closed: bool,
    fill: Option<&str>,
    stroke: Option<f32>,
    cap: &str,
) -> PageItem {
    Polygon {
        self_id: self_id(SAMPLE, "Polygon", i),
        item_transform: translate(x, y),
        fill_color: fill.map(str::to_string),
        stroke_color: stroke.map(|_| "Color/Black".to_string()),
        stroke_weight_pt: Some(stroke.unwrap_or(0.0)),
        extra_attrs: attrs(&[("EndCap", cap)]),
        subpaths: vec![PolygonSubPath::corners(pts, closed)],
        text_path: None,
    }
    .into()
}

/// Give every shape on the spread the shared shadow. The shape builders
/// write no `TransparencySetting`, so it is spliced in where InDesign
/// writes it: right after the item's `</Properties>`.
fn with_shadows(spread_xml: Vec<u8>) -> Vec<u8> {
    let mut xml = String::from_utf8(spread_xml).expect("utf-8");
    for tag in ["GraphicLine", "Polygon", "Oval"] {
        xml = xml.replace(
            &format!("</Properties></{tag}>"),
            &format!("</Properties>{SHADOW}</{tag}>"),
        );
    }
    xml.into_bytes()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let triangle = vec![(0.0, 100.0), (60.0, 0.0), (120.0, 100.0)];
    let items: Vec<PageItem> = vec![
        // Row 1 — lines.
        line(0, COLUMNS[0], 90.0, 6.0, "RoundEndCap", (140.0, 0.0)),
        line(1, COLUMNS[1] + 40.0, 90.0, 1.0, "ButtEndCap", (140.0, 0.0)),
        // Row 2 — a pen path and a diagonal line.
        polygon(
            0,
            COLUMNS[0],
            200.0,
            vec![(0.0, 60.0), (60.0, 0.0), (120.0, 60.0)],
            false,
            None,
            Some(6.0),
            "RoundEndCap",
        ),
        line(
            2,
            COLUMNS[1] + 40.0,
            200.0,
            6.0,
            "ProjectingEndCap",
            (120.0, 60.0),
        ),
        // Row 3 — the references.
        polygon(
            1,
            COLUMNS[0],
            360.0,
            triangle.clone(),
            true,
            None,
            Some(6.0),
            "ButtEndCap",
        ),
        polygon(
            2,
            COLUMNS[1],
            360.0,
            triangle,
            true,
            Some("Color/Grey40"),
            None,
            "ButtEndCap",
        ),
        Oval {
            self_id: self_id(SAMPLE, "Oval", 0),
            width_pt: 120.0,
            height_pt: 100.0,
            item_transform: translate(COLUMNS[2], 360.0),
            fill_color: Some("Color/Grey40".into()),
            stroke_color: Some("Color/Black".into()),
            stroke_weight_pt: Some(3.0),
            extra_attrs: Vec::new(),
        }
        .into(),
    ];

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
        with_shadows(write_spread(&Spread {
            self_id: spread_id.clone(),
            page_self_id: page_id,
            page_name: "line-shadows · lines · pen path · references".to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: items,
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        })),
    )];
    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id],
        spreads: vec![spread_id],
        stories: Vec::new(),
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
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
        stories: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader every shape carries the shadow,
    /// lines and the pen path included.
    #[test]
    fn every_shape_reads_back_with_its_shadow() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let s = &doc.spreads[0].spread;
        assert_eq!(s.graphic_lines.len(), 3);
        assert_eq!(s.polygons.len(), 3);
        assert_eq!(s.ovals.len(), 1);
        for ds in s
            .graphic_lines
            .iter()
            .map(|l| &l.drop_shadow)
            .chain(s.polygons.iter().map(|p| &p.drop_shadow))
            .chain(s.ovals.iter().map(|o| &o.drop_shadow))
        {
            let ds = ds.as_ref().expect("shadow read");
            assert_eq!((ds.x_offset, ds.y_offset, ds.size), (6.0, 6.0, 4.0));
        }
    }
}
