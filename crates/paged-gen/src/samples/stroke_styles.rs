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

//! `stroke-styles.idml` — InDesign's built-in dashed, dotted and hashed
//! stroke styles (R-14), asked of InDesign.
//!
//! InDesign does not stroke a dashed rectangle with one dash array: its PDF
//! draws each side as its own path with its own array, stretched so a whole
//! number of periods fits between the corners, and draws the corners
//! separately (`strokes-fills` p11–p14). This page measures that rule over
//! sizes, weights and open paths. One page, no text; each row is one style,
//! top to bottom:
//!
//! 1. `Dashed` (no `StrokeDashAndGap`), 2. `Canned Dashed 3x2`,
//! 3. `Canned Dashed 4x4`, 4. `Canned Dotted`, 5. `Japanese Dots`,
//! 6. `Left Slant Hash`, 7. `Right Slant Hash`, 8. `Straight Hash`,
//! 9. `White Diamond`.
//!
//! and each column one shape:
//!
//! * a — a 120 × 60 rectangle at 2 pt;
//! * b — a 137 × 61 rectangle at 5 pt (sides that are not multiples of
//!   any period);
//! * c — a 150 pt horizontal `<GraphicLine>` at 4 pt (an open path: no
//!   corners, two ends).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{GraphicLine, PageItem, PathPoint, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml, BUILTIN_STROKE_STYLES,
    },
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "stroke-styles";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
/// The styles, one per row, by their `StrokeStyle/$ID/<name>` names.
pub const STYLES: [&str; 9] = [
    "Dashed",
    "Canned Dashed 3x2",
    "Canned Dashed 4x4",
    "Canned Dotted",
    "Japanese Dots",
    "Left Slant Hash",
    "Right Slant Hash",
    "Straight Hash",
    "White Diamond",
];
const TOP_PT: f32 = 40.0;
const ROW_PT: f32 = 86.0;
/// Left edge of each column.
const COLUMNS: [f32; 3] = [40.0, 200.0, 380.0];

fn stroke_type(style: &str) -> Vec<(String, String)> {
    vec![("StrokeType".to_string(), format!("StrokeStyle/$ID/{style}"))]
}

/// The shared `Resources/Graphic.xml` declaring every built-in stroke
/// style, as InDesign's own Graphic.xml does: InDesign resolves a built-in
/// `StrokeType` only against its declaration in the package, and the
/// first export of this page, without them, came back solid throughout.
fn graphic_xml_with_builtins() -> Vec<u8> {
    let decls: String = BUILTIN_STROKE_STYLES
        .iter()
        .map(|name| format!(r#"<StrokeStyle Self="StrokeStyle/$ID/{name}" Name="$ID/{name}"/>"#))
        .collect();
    let xml = String::from_utf8(graphic_xml()).expect("utf-8");
    xml.replacen("</idPkg:Graphic>", &format!("{decls}</idPkg:Graphic>"), 1)
        .into_bytes()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let mut items: Vec<PageItem> = Vec::new();
    for (row, style) in STYLES.iter().enumerate() {
        let y = TOP_PT + row as f32 * ROW_PT;
        let seq = row as u32 * 3;
        let mut a = Rect::filled(
            self_id(SAMPLE, "Rectangle", seq),
            120.0,
            60.0,
            translate(COLUMNS[0], y),
        )
        .with_stroke("Color/Black", 2.0);
        a.fill_color = None;
        a.extra_attrs = stroke_type(style);
        items.push(a.into());
        let mut b = Rect::filled(
            self_id(SAMPLE, "Rectangle", seq + 1),
            137.0,
            61.0,
            translate(COLUMNS[1], y),
        )
        .with_stroke("Color/Black", 5.0);
        b.fill_color = None;
        b.extra_attrs = stroke_type(style);
        items.push(b.into());
        items.push(
            GraphicLine {
                self_id: self_id(SAMPLE, "GraphicLine", row as u32),
                item_transform: translate(COLUMNS[2], y + 30.0),
                stroke_color: Some("Color/Black".into()),
                stroke_weight_pt: Some(4.0),
                extra_attrs: stroke_type(style),
                points: vec![
                    PathPoint::corner((0.0, 0.0)),
                    PathPoint::corner((150.0, 0.0)),
                ],
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
            page_name: "stroke-styles · built-in dashes, dots and hashes".to_string(),
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
        graphic_xml: graphic_xml_with_builtins(),
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

    /// Every row's three items carry the row's style.
    #[test]
    fn the_fixture_reads_back_as_authored() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let s = &doc.spreads[0].spread;
        assert_eq!(s.rectangles.len(), 18);
        assert_eq!(s.graphic_lines.len(), 9);
        for (row, style) in STYLES.iter().enumerate() {
            let want = format!("StrokeStyle/$ID/{style}");
            assert_eq!(s.rectangles[row * 2].stroke_type.as_deref(), Some(&*want));
            assert_eq!(s.graphic_lines[row].stroke_type.as_deref(), Some(&*want));
        }
    }
}
