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

//! An object style's corners, as InDesign reads them (ADR 149;
//! `tools/indesign-export/object-style-corners-probe`). InDesign 20.0.1
//! read back, for a rectangle that states no corners of its own:
//!
//!   * the style's all-corner `CornerOption` / `CornerRadius` pair alone:
//!     square corners;
//!   * its four per-corner attributes alone: those corners;
//!   * both, disagreeing: the per-corner ones;
//!   * per-corner options with no radii, next to `CornerRadius="30"`: the
//!     options at 12 pt.
//!
//! Each case is pinned by equivalence: the styled rectangle must paint
//! exactly what a rectangle stating those corners itself paints.

use paged_compose::{DisplayCommand, PathSegment};
use paged_model::{CornerOption, CornerSpec, ObjectStyleCategories, ObjectStyleDef, StyleSheet};

const BOX_PATH: &str = r#"<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
<PathPointType Anchor="50 50" LeftDirection="50 50" RightDirection="50 50"/>
<PathPointType Anchor="50 250" LeftDirection="50 250" RightDirection="50 250"/>
<PathPointType Anchor="250 250" LeftDirection="250 250" RightDirection="250 250"/>
<PathPointType Anchor="250 50" LeftDirection="250 50" RightDirection="250 50"/>
</PathPointArray></GeometryPathType></PathGeometry></Properties>"#;

fn document(rect_attrs: &str, styles: StyleSheet) -> paged_scene::Document {
    use std::collections::HashMap;
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="50 50 250 250" ItemTransform="1 0 0 1 0 0" FillColor="Color/Black" {rect_attrs}>{BOX_PATH}</Rectangle>
</Spread></idPkg:Spread>"#
    );
    let spread = idml_import::parse_spread(xml.as_bytes()).expect("parse spread");
    paged_scene::Document {
        designmap: paged_model::DesignMap::default(),
        palette: paged_model::Graphic::default(),
        spreads: vec![paged_scene::ParsedSpread {
            src: "Spreads/syn.xml".to_string(),
            spread,
        }],
        stories: Vec::new(),
        master_spreads: HashMap::new(),
        frame_for_story: HashMap::new(),
        text_frame_index: HashMap::new(),
        styles,
        anchors: Vec::new(),
    }
}

/// The rectangle's fill outline.
fn outline(rect_attrs: &str, styles: StyleSheet) -> Vec<PathSegment> {
    let options = paged_renderer::pipeline::PipelineOptions::default();
    let built = paged_renderer::pipeline::build_document(&document(rect_attrs, styles), &options)
        .expect("build");
    let page = &built.pages[0];
    page.list
        .commands
        .iter()
        .find_map(|cmd| match cmd {
            DisplayCommand::FillPath { path_id, .. } => {
                page.list.paths.get(*path_id).map(|p| p.segments.clone())
            }
            _ => None,
        })
        .expect("the rectangle paints a fill")
}

/// What a rectangle stating `attrs` itself paints, with no style.
fn own(attrs: &str) -> Vec<PathSegment> {
    outline(attrs, StyleSheet::default())
}

fn styled(styles: Vec<ObjectStyleDef>) -> Vec<PathSegment> {
    let mut sheet = StyleSheet::default();
    for s in styles {
        sheet.object_styles.insert(s.self_id.clone(), s);
    }
    outline(r#"AppliedObjectStyle="ObjectStyle/Corners""#, sheet)
}

fn style(corners: [CornerSpec; 4]) -> ObjectStyleDef {
    ObjectStyleDef {
        self_id: "ObjectStyle/Corners".into(),
        name: Some("Corners".into()),
        corners,
        ..Default::default()
    }
}

fn spec(option: CornerOption, radius: Option<f32>) -> CornerSpec {
    CornerSpec {
        option: Some(option),
        radius,
    }
}

const ROUNDED_20: &str = r#"TopLeftCornerOption="RoundedCorner" TopLeftCornerRadius="20"
  TopRightCornerOption="RoundedCorner" TopRightCornerRadius="20"
  BottomRightCornerOption="RoundedCorner" BottomRightCornerRadius="20"
  BottomLeftCornerOption="RoundedCorner" BottomLeftCornerRadius="20""#;

#[test]
fn a_styles_corner_pair_alone_draws_square_corners() {
    let s = ObjectStyleDef {
        corner_option: Some("RoundedCorner".into()),
        corner_radius: Some(20.0),
        ..style(Default::default())
    };
    assert_eq!(styled(vec![s]), own(""));
}

#[test]
fn a_styles_per_corner_attributes_draw_its_corners() {
    let s = style([spec(CornerOption::Rounded, Some(20.0)); 4]);
    let drawn = styled(vec![s]);
    assert_ne!(drawn, own(""), "the style's rounded corners were not drawn");
    assert_eq!(drawn, own(ROUNDED_20));
}

#[test]
fn the_per_corner_attributes_win_over_a_disagreeing_pair() {
    let s = ObjectStyleDef {
        corner_option: Some("InsetCorner".into()),
        corner_radius: Some(30.0),
        ..style([spec(CornerOption::Rounded, Some(20.0)); 4])
    };
    assert_eq!(styled(vec![s]), own(ROUNDED_20));
}

#[test]
fn a_per_corner_option_without_a_radius_is_12_pt_not_the_pairs() {
    let s = ObjectStyleDef {
        corner_radius: Some(30.0),
        ..style([spec(CornerOption::Rounded, None); 4])
    };
    let rounded_12 = r#"TopLeftCornerOption="RoundedCorner" TopLeftCornerRadius="12"
      TopRightCornerOption="RoundedCorner" TopRightCornerRadius="12"
      BottomRightCornerOption="RoundedCorner" BottomRightCornerRadius="12"
      BottomLeftCornerOption="RoundedCorner" BottomLeftCornerRadius="12""#;
    assert_eq!(styled(vec![s]), own(rounded_12));
}

/// Each corner's option and radius cascade through `BasedOn` on their
/// own: a child that restates one radius keeps its parent's options and
/// other radii.
#[test]
fn each_corner_cascades_through_based_on() {
    let parent = ObjectStyleDef {
        self_id: "ObjectStyle/Parent".into(),
        ..style([spec(CornerOption::Rounded, Some(20.0)); 4])
    };
    let mut child = style(Default::default());
    child.based_on = Some("ObjectStyle/Parent".into());
    child.corners[0].radius = Some(5.0);
    let expected = r#"TopLeftCornerOption="RoundedCorner" TopLeftCornerRadius="5"
      TopRightCornerOption="RoundedCorner" TopRightCornerRadius="20"
      BottomRightCornerOption="RoundedCorner" BottomRightCornerRadius="20"
      BottomLeftCornerOption="RoundedCorner" BottomLeftCornerRadius="20""#;
    assert_eq!(styled(vec![parent, child]), own(expected));
}

#[test]
fn a_style_with_its_corner_category_off_passes_no_corners() {
    let s = ObjectStyleDef {
        enable: ObjectStyleCategories {
            stroke_and_corner_options: Some(false),
            ..Default::default()
        },
        ..style([spec(CornerOption::Rounded, Some(20.0)); 4])
    };
    assert_eq!(styled(vec![s]), own(""));
}

#[test]
fn a_rectangle_stating_its_own_corners_ignores_its_styles() {
    let mut sheet = StyleSheet::default();
    let s = style([spec(CornerOption::Bevel, Some(30.0)); 4]);
    sheet.object_styles.insert(s.self_id.clone(), s);
    let attrs = format!(r#"AppliedObjectStyle="ObjectStyle/Corners" {ROUNDED_20}"#);
    assert_eq!(outline(&attrs, sheet), own(ROUNDED_20));
}

/// The style as InDesign 20.0.1 exported it (corner attributes only), read
/// by the real stylesheet parser: its four per-corner attributes reach the
/// rectangle; with only its pair left, nothing does.
#[test]
fn indesigns_own_spelling_reaches_the_rectangle() {
    const PER_CORNER: &str = r#"TopLeftCornerOption="RoundedCorner" TopRightCornerOption="RoundedCorner" BottomLeftCornerOption="RoundedCorner" BottomRightCornerOption="RoundedCorner" TopLeftCornerRadius="20" TopRightCornerRadius="20" BottomLeftCornerRadius="20" BottomRightCornerRadius="20""#;
    const PAIR: &str = r#"CornerRadius="20" CornerOption="RoundedCorner""#;
    let sheet = |attrs: &str| {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<RootObjectStyleGroup Self="u7a">
<ObjectStyle Self="ObjectStyle/Corners" Name="Corners" EnableFill="true" EnableStroke="true" EnableStrokeAndCornerOptions="true" {attrs}/>
</RootObjectStyleGroup>
</idPkg:Styles>"#
        );
        idml_import::parse_stylesheet(xml.as_bytes()).expect("parse styles")
    };
    let applied = r#"AppliedObjectStyle="ObjectStyle/Corners""#;
    assert_eq!(
        outline(applied, sheet(&format!("{PER_CORNER} {PAIR}"))),
        own(ROUNDED_20)
    );
    assert_eq!(outline(applied, sheet(PER_CORNER)), own(ROUNDED_20));
    assert_eq!(outline(applied, sheet(PAIR)), own(""));
}
