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

//! C-18 — which kinds' corner attributes REACH THE RENDERER.
//!
//! The gap census found the corner vocabulary on Group / GraphicLine /
//! Oval / TextFrame as well as Rectangle / Polygon. All six now parse,
//! write and mutate it, but only a shape with a real outline can render
//! it. This file pins that split so a later "symmetry" pass cannot
//! quietly start drawing corners on an ellipse — and so the one kind
//! that DOES render them can't drift from the calibrated rectangle lane.
//!
//!   * TextFrame — RENDERS. Its body is the same outline a `<Rectangle>`
//!     paints, so a `CornerOption` cuts it identically. Pinned by
//!     equivalence against a rectangle with the same box and attrs.
//!   * Oval / GraphicLine / Group — DO NOT render. Pinned by showing the
//!     display list is unchanged with and without the attributes.
//!
//! Everything drives the REAL parser (`idml_import::parse_spread`), so
//! the attribute spellings under test are the ones InDesign writes.

use paged_compose::{DisplayCommand, PathSegment};

/// The box every shape under test occupies, in spread/pixel space.
const BOX: &str = "50 50 250 250";
/// The four-anchor `<PathGeometry>` for that box — real InDesign exports
/// always carry one, and `text_frame_is_rect_path` recognises it as the
/// plain rectangular panel.
const BOX_PATH: &str = r#"<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
<PathPointType Anchor="50 50" LeftDirection="50 50" RightDirection="50 50"/>
<PathPointType Anchor="50 250" LeftDirection="50 250" RightDirection="50 250"/>
<PathPointType Anchor="250 250" LeftDirection="250 250" RightDirection="250 250"/>
<PathPointType Anchor="250 50" LeftDirection="250 50" RightDirection="250 50"/>
</PathPointArray></GeometryPathType></PathGeometry></Properties>"#;

/// Corner attributes as InDesign spells them, 20 pt rounded on all four.
const ROUNDED: &str = r#"CornerOption="RoundedCorner" CornerRadius="20"
  TopLeftCornerOption="RoundedCorner" TopLeftCornerRadius="20"
  TopRightCornerOption="RoundedCorner" TopRightCornerRadius="20"
  BottomRightCornerOption="RoundedCorner" BottomRightCornerRadius="20"
  BottomLeftCornerOption="RoundedCorner" BottomLeftCornerRadius="20""#;

/// Build a one-page document whose spread carries `items` verbatim.
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
        palette: paged_model::Graphic::default(),
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

fn built(items: &str) -> paged_renderer::pipeline::BuiltDocument {
    let options = paged_renderer::pipeline::PipelineOptions::default();
    paged_renderer::pipeline::build_document(&document(items), &options).expect("build")
}

/// Every `FillPath` / `StrokePath` command's segments, in emit order.
fn path_segments(page: &paged_renderer::pipeline::BuiltPage) -> Vec<Vec<PathSegment>> {
    page.list
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            DisplayCommand::FillPath { path_id, .. }
            | DisplayCommand::StrokePath { path_id, .. } => {
                page.list.paths.get(*path_id).map(|p| p.segments.clone())
            }
            _ => None,
        })
        .collect()
}

/// The whole command stream, rendered for comparison. Used by the
/// "renders nothing" tests: a kind that ignores its corner attributes
/// must produce an identical list with and without them.
fn command_dump(items: &str) -> String {
    format!("{:?}", built(items).pages[0].list.commands)
}

fn text_frame(attrs: &str) -> String {
    format!(
        r#"<TextFrame Self="tf1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" FillColor="Color/Black" {attrs}>{BOX_PATH}</TextFrame>"#
    )
}

fn rectangle(attrs: &str) -> String {
    format!(
        r#"<Rectangle Self="r1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" FillColor="Color/Black" {attrs}>{BOX_PATH}</Rectangle>"#
    )
}

/// C-18 — a rectangular TEXT FRAME with corner attrs paints the SAME
/// outline a `<Rectangle>` with the same box and attrs does.
///
/// This is the whole claim for the one kind C-18 renders: not "a text
/// frame draws something roundish" but "it draws exactly what the
/// already-calibrated rectangle lane draws". If the two ever diverge,
/// one of them is wrong.
#[test]
fn c18_text_frame_corner_path_equals_the_rectangle_path() {
    let tf = path_segments(&built(&text_frame(ROUNDED)).pages[0]);
    let rect = path_segments(&built(&rectangle(ROUNDED)).pages[0]);
    assert!(!tf.is_empty(), "the rounded text frame emitted no path");
    assert!(!rect.is_empty(), "the rounded rectangle emitted no path");
    assert_eq!(
        tf[0], rect[0],
        "a rounded text frame must paint the identical outline a rounded \
         rectangle paints",
    );
}

/// …and the effect is a real CURVE, not a relabelled square. Guards the
/// equality above against passing because both sides degenerated.
#[test]
fn c18_text_frame_rounded_corners_emit_cubics() {
    let segs = path_segments(&built(&text_frame(ROUNDED)).pages[0]);
    let cubics = segs
        .iter()
        .flatten()
        .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
        .count();
    assert!(
        cubics >= 4,
        "four rounded corners must emit at least four cubic segments, got {cubics}",
    );
}

/// …and without the attributes a text frame stays on the cheap
/// UNIT-RECT primitive (a 0..1 box scaled by `Transform::for_rect_in`),
/// exactly as before C-18. The corner lane must not tax every ordinary
/// text panel with a bespoke interned outline.
#[test]
fn c18_plain_text_frame_still_emits_no_corner_path() {
    let plain = path_segments(&built(&text_frame("")).pages[0]);
    let cubics = plain
        .iter()
        .flatten()
        .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
        .count();
    assert_eq!(
        cubics, 0,
        "a text frame with no corner attrs must emit no curve: {plain:#?}",
    );
    // Positively: it is the unit rect, not a 50..250 corner path.
    assert!(
        plain.iter().flatten().all(|s| match *s {
            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } =>
                (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
            _ => true,
        }),
        "expected the unit-rect primitive, got {plain:#?}",
    );
}

/// C-18 — the legacy global `CornerOption` / `CornerRadius` pair drives a
/// text frame too, not only the four per-corner slots.
#[test]
fn c18_text_frame_honours_the_global_corner_pair() {
    let global = path_segments(
        &built(&text_frame(
            r#"CornerOption="RoundedCorner" CornerRadius="20""#,
        ))
        .pages[0],
    );
    let per_corner = path_segments(&built(&text_frame(ROUNDED)).pages[0]);
    assert!(!global.is_empty(), "the global pair emitted no path");
    assert_eq!(
        global, per_corner,
        "the global pair and four equal per-corner slots must agree",
    );
}

/// C-18 — an OVAL's corner attributes are stored, mutable and
/// round-tripped, and render NOTHING.
///
/// Evidence, not assumption: an ellipse is emitted from `bounds` as the
/// inscribed ellipse and `paged_model::Oval` carries no `anchors` at
/// all, so there is no vertex for a corner effect to cut — and InDesign
/// draws nothing for Corner Options on an ellipse either.
#[test]
fn c18_oval_corner_attributes_do_not_change_the_display_list() {
    let oval = |attrs: &str| {
        format!(
            r#"<Oval Self="ov1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" FillColor="Color/Black" {attrs}/>"#
        )
    };
    let plain = command_dump(&oval(""));
    assert!(!plain.is_empty(), "the oval must have painted something");
    assert_eq!(
        plain,
        command_dump(&oval(ROUNDED)),
        "corner attributes must not alter an oval's rendering",
    );
}

/// C-18 — same for a GRAPHIC LINE. An open, stroke-only contour has no
/// enclosed corner; its interior joins are shaped by `EndJoin` /
/// `MiterLimit`, not the corner vocabulary. (The corpus agrees: 21 lines
/// carry corner RADII and not one carries a `CornerOption`, so the
/// radius-only spelling below is the real-world case.)
#[test]
fn c18_graphic_line_corner_attributes_do_not_change_the_display_list() {
    let line = |attrs: &str| {
        format!(
            r#"<GraphicLine Self="gl1" GeometricBounds="{BOX}" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="2" {attrs}/>"#
        )
    };
    let plain = command_dump(&line(""));
    assert!(!plain.is_empty(), "the line must have painted something");
    // Both the corpus spelling (radii only) and the full vocabulary.
    assert_eq!(
        plain,
        command_dump(&line(
            r#"CornerRadius="99.21259842519686" TopLeftCornerRadius="99.21259842519686""#
        )),
        "corner radii must not alter a graphic line's rendering",
    );
    assert_eq!(
        plain,
        command_dump(&line(ROUNDED)),
        "even a full corner option set must not alter a line's rendering",
    );
}

/// C-18 — and for a GROUP, which has no outline of its own: it paints
/// nothing, its members paint themselves with their OWN corner
/// attributes. A group-level corner must not leak onto them.
#[test]
fn c18_group_corner_attributes_do_not_change_the_display_list() {
    let group = |attrs: &str| {
        format!(
            r#"<Group Self="g1" ItemTransform="1 0 0 1 0 0" {attrs}>{}</Group>"#,
            rectangle("")
        )
    };
    let plain = command_dump(&group(""));
    assert!(
        plain.contains("FillPath"),
        "the group's member must have painted: {plain}",
    );
    assert_eq!(
        plain,
        command_dump(&group(ROUNDED)),
        "a group's corner attributes must not alter its members' painting",
    );
}
