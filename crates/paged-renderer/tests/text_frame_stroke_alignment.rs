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

//! A text frame's stroke paints where its `StrokeAlignment` says — the
//! same outline a `<Rectangle>` / `<Polygon>` with the same geometry and
//! attributes strokes: moved in by half the weight (Inside), out by half
//! (Outside), or the frame edge itself (Center, or no attribute).
//!
//! The text side of the same attribute (how far the stroke insets the
//! text) is pinned line by line against InDesign in
//! `stroke_inset_pipeline.rs`. Everything here drives the real parser,
//! so the spellings are the ones InDesign writes.

use paged_compose::{DisplayCommand, PathSegment};

const BOX: &str = "50 50 250 250";
const BOX_PATH: &str = r#"<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
<PathPointType Anchor="50 50" LeftDirection="50 50" RightDirection="50 50"/>
<PathPointType Anchor="50 250" LeftDirection="50 250" RightDirection="50 250"/>
<PathPointType Anchor="250 250" LeftDirection="250 250" RightDirection="250 250"/>
<PathPointType Anchor="250 50" LeftDirection="250 50" RightDirection="250 50"/>
</PathPointArray></GeometryPathType></PathGeometry></Properties>"#;
/// A triangle, so the text frame is a pathed (`Polygon`) one.
const TRIANGLE_PATH: &str = r#"<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
<PathPointType Anchor="50 250" LeftDirection="50 250" RightDirection="50 250"/>
<PathPointType Anchor="150 50" LeftDirection="150 50" RightDirection="150 50"/>
<PathPointType Anchor="250 250" LeftDirection="250 250" RightDirection="250 250"/>
</PathPointArray></GeometryPathType></PathGeometry></Properties>"#;

const STROKE: &str = r#"StrokeColor="Color/Black" StrokeWeight="10""#;

fn document(items: &str) -> paged_scene::Document {
    use std::collections::HashMap;
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 400 400" ItemTransform="1 0 0 1 0 0"/>
{items}
</Spread></idPkg:Spread>"#
    );
    let spread = idml_import::parse_spread(xml.as_bytes()).expect("parse spread");
    paged_scene::Document {
        designmap: paged_model::DesignMap::default(),
        palette: idml_import::parse_graphic(
            br#"<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Color Self="Color/Black" Name="Black" Space="CMYK" ColorValue="0 0 0 100"/></idPkg:Graphic>"#,
        )
        .expect("graphic"),
        spreads: vec![paged_scene::ParsedSpread {
            src: "Spreads/syn.xml".to_string(),
            spread,
        }],
        stories: Vec::new(),
        master_spreads: HashMap::new(),
        frame_for_story: HashMap::new(),
        text_frame_index: HashMap::new(),
        styles: paged_model::StyleSheet::default(),
        anchors: Vec::new(),
    }
}

/// Every stroked outline on the page, in page coordinates (each path's
/// points through its command's transform).
fn stroked_outlines(items: &str) -> Vec<Vec<(f32, f32)>> {
    let options = paged_renderer::pipeline::PipelineOptions::default();
    let built =
        paged_renderer::pipeline::build_document(&document(items), &options).expect("build");
    let page = &built.pages[0];
    page.list
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            DisplayCommand::StrokePath {
                path_id, transform, ..
            } => page.list.paths.get(*path_id).map(|p| {
                p.segments
                    .iter()
                    .filter_map(|s| match *s {
                        PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => {
                            Some(transform.apply(x, y))
                        }
                        PathSegment::QuadTo { x, y, .. } | PathSegment::CubicTo { x, y, .. } => {
                            Some(transform.apply(x, y))
                        }
                        _ => None,
                    })
                    .collect()
            }),
            _ => None,
        })
        .collect()
}

fn bbox(points: &[(f32, f32)]) -> [f32; 4] {
    points.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |b, &(x, y)| [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
    )
}

fn alignment_attr(alignment: Option<&str>) -> String {
    alignment
        .map(|a| format!(r#"StrokeAlignment="{a}""#))
        .unwrap_or_default()
}

fn one_stroke(items: &str) -> Vec<(f32, f32)> {
    let mut all = stroked_outlines(items);
    assert_eq!(all.len(), 1, "one stroked outline expected: {all:?}");
    all.remove(0)
}

#[test]
fn a_rectangular_text_frame_strokes_where_its_alignment_says() {
    for (alignment, shift) in [
        (Some("InsideAlignment"), 5.0),
        (Some("OutsideAlignment"), -5.0),
        (Some("CenterAlignment"), 0.0),
        (None, 0.0),
    ] {
        let a = alignment_attr(alignment);
        let tf = one_stroke(&format!(
            r#"<TextFrame Self="tf1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" {STROKE} {a}>{BOX_PATH}</TextFrame>"#
        ));
        let got = bbox(&tf);
        let want = [50.0 + shift, 50.0 + shift, 250.0 - shift, 250.0 - shift];
        for k in 0..4 {
            assert!(
                (got[k] - want[k]).abs() < 1e-3,
                "{alignment:?}: stroked outline {got:?}, want {want:?}"
            );
        }
        let rect = one_stroke(&format!(
            r#"<Rectangle Self="r1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" {STROKE} {a}>{BOX_PATH}</Rectangle>"#
        ));
        assert_eq!(
            bbox(&rect),
            got,
            "{alignment:?}: a text frame strokes the outline a rectangle does"
        );
    }
}

#[test]
fn a_rounded_text_frame_strokes_the_rounded_rectangles_aligned_outline() {
    const ROUNDED: &str = r#"CornerOption="RoundedCorner" CornerRadius="20""#;
    for alignment in ["InsideAlignment", "OutsideAlignment"] {
        let a = alignment_attr(Some(alignment));
        let tf = one_stroke(&format!(
            r#"<TextFrame Self="tf1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" {STROKE} {ROUNDED} {a}>{BOX_PATH}</TextFrame>"#
        ));
        let rect = one_stroke(&format!(
            r#"<Rectangle Self="r1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" {STROKE} {ROUNDED} {a}>{BOX_PATH}</Rectangle>"#
        ));
        assert_eq!(tf, rect, "{alignment}");
        let centred = one_stroke(&format!(
            r#"<TextFrame Self="tf1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" {STROKE} {ROUNDED}>{BOX_PATH}</TextFrame>"#
        ));
        assert_ne!(tf, centred, "{alignment} moves the rounded outline");
    }
}

#[test]
fn a_pathed_text_frame_strokes_the_polygons_aligned_outline() {
    for alignment in ["InsideAlignment", "OutsideAlignment"] {
        let a = alignment_attr(Some(alignment));
        let tf = one_stroke(&format!(
            r#"<TextFrame Self="tf1" ItemTransform="1 0 0 1 0 0" {STROKE} {a}>{TRIANGLE_PATH}</TextFrame>"#
        ));
        let poly = one_stroke(&format!(
            r#"<Polygon Self="tf1" ItemTransform="1 0 0 1 0 0" {STROKE} {a}>{TRIANGLE_PATH}</Polygon>"#
        ));
        assert_eq!(tf, poly, "{alignment}");
        let centred = one_stroke(&format!(
            r#"<TextFrame Self="tf1" ItemTransform="1 0 0 1 0 0" {STROKE}>{TRIANGLE_PATH}</TextFrame>"#
        ));
        let (t, c) = (bbox(&tf), bbox(&centred));
        if alignment == "InsideAlignment" {
            assert!(
                t[0] > c[0] && t[2] < c[2] && t[3] < c[3],
                "{t:?} inside {c:?}"
            );
        } else {
            assert!(
                t[0] < c[0] && t[2] > c[2] && t[3] > c[3],
                "{t:?} outside {c:?}"
            );
        }
    }
}
