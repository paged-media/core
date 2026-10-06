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

//! `line-ends.idml` — a pen path's stroke END CAP and LINE ENDS (C-62),
//! asked of InDesign.
//!
//! A pen or pencil path is a `<Polygon>` whose contour is open; IDML
//! writes `EndCap` and `LeftLineEnd` / `RightLineEnd` (+ scales) on it.
//! One page, no text, so the reference measures strokes only:
//!
//! * row 1 — three open polygons at 14 pt: `ButtEndCap`, `RoundEndCap`,
//!   `ProjectingEndCap`;
//! * row 2 — open polygons at 3 pt with line ends: circle + triangle,
//!   barbed + simple-wide, square at 150 % + triangle at 50 %;
//! * row 3 — a compound path of two OPEN contours with line ends (does
//!   each contour take them?), a CLOSED triangle with line ends set (it
//!   should draw none), and a round-capped 8 pt path ending in a
//!   triangle;
//! * row 4 — `<GraphicLine>`s: 14 pt round and projecting caps, and a
//!   3 pt round-capped line with a line end at each end;
//! * row 5 — dashed ovals at 6 pt in the three caps (the cap shapes the
//!   dash ends of a closed outline).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{GraphicLine, Oval, PageItem, PathPoint, Polygon, PolygonSubPath},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "line-ends";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
/// Left edge of each of the three columns.
const COLUMNS: [f32; 3] = [70.0, 240.0, 410.0];

fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// An open zig-zag 120 pt wide: the ends point along two different
/// directions, so a misoriented line end is visible.
fn zigzag() -> Vec<(f32, f32)> {
    vec![(0.0, 30.0), (60.0, 0.0), (120.0, 30.0)]
}

struct Pen {
    x: f32,
    y: f32,
    weight: f32,
    contours: Vec<(Vec<(f32, f32)>, bool)>,
    attrs: Vec<(String, String)>,
}

fn pens() -> Vec<Pen> {
    let open = |pts: Vec<(f32, f32)>| vec![(pts, false)];
    let mut out = Vec::new();
    // Row 1 — the three caps on a heavy open path.
    for (x, cap) in COLUMNS
        .iter()
        .zip(["ButtEndCap", "RoundEndCap", "ProjectingEndCap"])
    {
        out.push(Pen {
            x: *x,
            y: 70.0,
            weight: 14.0,
            contours: open(zigzag()),
            attrs: attrs(&[("EndCap", cap)]),
        });
    }
    // Row 2 — line ends on a thin open path.
    for (x, ends) in COLUMNS.iter().zip([
        &[
            ("LeftLineEnd", "CircleSolidArrowHead"),
            ("RightLineEnd", "TriangleArrowHead"),
        ][..],
        &[
            ("LeftLineEnd", "BarbedArrowHead"),
            ("RightLineEnd", "SimpleWideArrowHead"),
        ][..],
        &[
            ("LeftLineEnd", "SquareArrowHead"),
            ("RightLineEnd", "TriangleArrowHead"),
            ("LeftArrowHeadScale", "150"),
            ("RightArrowHeadScale", "50"),
        ][..],
    ]) {
        out.push(Pen {
            x: *x,
            y: 190.0,
            weight: 3.0,
            contours: open(zigzag()),
            attrs: attrs(ends),
        });
    }
    // Row 3 — two open contours; a closed one; a round cap under a line end.
    let ends = [
        ("LeftLineEnd", "TriangleArrowHead"),
        ("RightLineEnd", "CircleSolidArrowHead"),
    ];
    out.push(Pen {
        x: COLUMNS[0],
        y: 310.0,
        weight: 3.0,
        contours: vec![
            (vec![(0.0, 0.0), (120.0, 0.0)], false),
            (vec![(0.0, 40.0), (120.0, 40.0)], false),
        ],
        attrs: attrs(&ends),
    });
    out.push(Pen {
        x: COLUMNS[1],
        y: 310.0,
        weight: 3.0,
        contours: vec![(vec![(0.0, 50.0), (60.0, 0.0), (120.0, 50.0)], true)],
        attrs: attrs(&ends),
    });
    out.push(Pen {
        x: COLUMNS[2],
        y: 310.0,
        weight: 8.0,
        contours: open(zigzag()),
        attrs: attrs(&[
            ("EndCap", "RoundEndCap"),
            ("RightLineEnd", "TriangleArrowHead"),
        ]),
    });
    out
}

/// The shared `Resources/Graphic.xml` plus the built-in
/// `<StrokeStyle Self="StrokeStyle/$ID/Dashed">` InDesign declares there.
/// Without it InDesign 20.0.1 resolves `StrokeType="StrokeStyle/$ID/Dashed"`
/// to nothing and draws the stroke SOLID — measured on this fixture's first
/// export, where all three ovals came back solid. The shared generator
/// declares no built-in stroke style at all; widening it would change every
/// fixture's reference, so it is reported rather than done here.
fn graphic_xml_with_dashed() -> Vec<u8> {
    let xml = String::from_utf8(graphic_xml()).expect("utf-8");
    xml.replacen(
        "</idPkg:Graphic>",
        r#"<StrokeStyle Self="StrokeStyle/$ID/Dashed" Name="$ID/Dashed"/></idPkg:Graphic>"#,
        1,
    )
    .into_bytes()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let mut items: Vec<PageItem> = Vec::new();
    for (i, pen) in pens().into_iter().enumerate() {
        items.push(
            Polygon {
                self_id: self_id(SAMPLE, "Polygon", i as u32),
                item_transform: translate(pen.x, pen.y),
                fill_color: None,
                stroke_color: Some("Color/Black".into()),
                stroke_weight_pt: Some(pen.weight),
                extra_attrs: pen.attrs,
                subpaths: pen
                    .contours
                    .into_iter()
                    .map(|(pts, closed)| PolygonSubPath::corners(pts, closed))
                    .collect(),
                text_path: None,
            }
            .into(),
        );
    }
    // Row 4 — lines.
    let lines: [(f32, &[(&str, &str)]); 3] = [
        (14.0, &[("EndCap", "RoundEndCap")]),
        (14.0, &[("EndCap", "ProjectingEndCap")]),
        (
            3.0,
            &[
                ("EndCap", "RoundEndCap"),
                ("LeftLineEnd", "CircleSolidArrowHead"),
                ("RightLineEnd", "BarbedArrowHead"),
            ],
        ),
    ];
    for (i, (x, (weight, line_attrs))) in COLUMNS.iter().zip(lines).enumerate() {
        items.push(
            GraphicLine {
                self_id: self_id(SAMPLE, "GraphicLine", i as u32),
                item_transform: translate(*x, 450.0),
                stroke_color: Some("Color/Black".into()),
                stroke_weight_pt: Some(weight),
                extra_attrs: attrs(line_attrs),
                points: vec![
                    PathPoint::corner((0.0, 0.0)),
                    PathPoint::corner((120.0, 0.0)),
                ],
            }
            .into(),
        );
    }
    // Row 5 — dashed ellipses; the cap shapes every dash end.
    for (i, (x, cap)) in COLUMNS
        .iter()
        .zip(["ButtEndCap", "RoundEndCap", "ProjectingEndCap"])
        .enumerate()
    {
        items.push(
            Oval {
                self_id: self_id(SAMPLE, "Oval", i as u32),
                width_pt: 120.0,
                height_pt: 80.0,
                item_transform: translate(*x, 540.0),
                fill_color: None,
                stroke_color: Some("Color/Black".into()),
                stroke_weight_pt: Some(6.0),
                // InDesign's own spelling of a custom dash (its IDML export
                // of the same oval, 2026-10-03): the `Dashed` type plus a
                // per-item `StrokeDashAndGap`. See `graphic_xml_with_dashed`
                // for the resource it also needs.
                extra_attrs: attrs(&[
                    ("StrokeType", "StrokeStyle/$ID/Dashed"),
                    ("StrokeDashAndGap", "14 10"),
                    ("EndCap", cap),
                ]),
            }
            .into(),
        );
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
            page_name: "line-ends · caps · line ends · lines · dashed ovals".to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: items,
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        }),
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
        graphic_xml: graphic_xml_with_dashed(),
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

    /// The fixture says what it claims: through the engine's own reader,
    /// every pen path carries its cap / line ends and stays open (or,
    /// for the triangle, closed), and the lines and ovals carry caps.
    #[test]
    fn the_fixture_reads_back_as_authored() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let s = &doc.spreads[0].spread;
        let caps: Vec<_> = s.polygons[..3]
            .iter()
            .map(|p| p.end_cap.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(caps, ["ButtEndCap", "RoundEndCap", "ProjectingEndCap"]);
        assert!(s.polygons[..3].iter().all(|p| p.subpath_open == [true]));
        let row2 = &s.polygons[5];
        assert_eq!(row2.start_arrow, idml_import::ArrowheadType::Square);
        assert_eq!(
            (row2.start_arrow_scale, row2.end_arrow_scale),
            (150.0, 50.0)
        );
        assert_eq!(s.polygons[6].subpath_open, [true, true]);
        // A closed single contour reads as "no open flags" (all closed).
        assert!(s.polygons[7].subpath_open.iter().all(|open| !open));
        assert_eq!(
            s.polygons[7].end_arrow,
            idml_import::ArrowheadType::CircleSolid
        );
        assert_eq!(s.graphic_lines.len(), 3);
        assert_eq!(s.graphic_lines[0].end_cap.as_deref(), Some("RoundEndCap"));
        assert_eq!(
            s.graphic_lines[2].end_arrow,
            idml_import::ArrowheadType::Barbed
        );
        assert_eq!(s.ovals[2].end_cap.as_deref(), Some("ProjectingEndCap"));
    }
}
