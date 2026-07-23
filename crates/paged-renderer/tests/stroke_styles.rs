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

//! W1.2 stroke-STYLE integration tests. Build a one-rectangle IDML whose
//! `StrokeType` references a custom `<…StrokeStyle>` and assert the
//! `build_document` display list carries the expected `StrokePath`
//! commands: N rules for a striped stroke, a gap-colour under-stroke for
//! a gap-coloured dash, a sine ribbon for a wavy stroke, and inward /
//! outward bounds shifts for stroke alignment.

use std::io::Write;

use paged_compose::{DisplayCommand, PathSegment};
use paged_renderer::{pipeline, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// Build a single-page IDML with one 200×100 rectangle at inner origin
/// (no ItemTransform), the supplied `<Rectangle>` attributes, and the
/// supplied custom `<…StrokeStyle>` resource XML injected into
/// `Resources/Styles.xml`.
fn build_idml(rect_attrs: &str, stroke_style_xml: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();

    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Styles src="Resources/Styles.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Black" Name="Black" Space="CMYK" ColorValue="0 0 0 100"/>
    <Color Self="Color/Cyan" Name="Cyan" Space="CMYK" ColorValue="100 0 0 0"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  {stroke_style_xml}
</idPkg:Styles>"#
    );
    zip.start_file("Resources/Styles.xml", deflated).unwrap();
    zip.write_all(styles.as_bytes()).unwrap();

    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 300"/>
    <Rectangle Self="r1" GeometricBounds="0 0 100 200" {rect_attrs}>
      <Properties>
        <PathGeometry>
          <GeometryPathType PathOpen="false">
            <PathPointArray>
              <PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/>
              <PathPointType Anchor="200 0" LeftDirection="200 0" RightDirection="200 0"/>
              <PathPointType Anchor="200 100" LeftDirection="200 100" RightDirection="200 100"/>
              <PathPointType Anchor="0 100" LeftDirection="0 100" RightDirection="0 100"/>
            </PathPointArray>
          </GeometryPathType>
        </PathGeometry>
      </Properties>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread.as_bytes()).unwrap();

    zip.finish().unwrap().into_inner()
}

fn built_commands(bytes: &[u8]) -> Vec<DisplayCommand> {
    let document = idml_import::import_idml_doc(bytes).unwrap();
    let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
    built.pages[0].list.commands.clone()
}

fn stroke_paths(cmds: &[DisplayCommand]) -> Vec<&DisplayCommand> {
    cmds.iter()
        .filter(|c| matches!(c, DisplayCommand::StrokePath { .. }))
        .collect()
}

/// Bounds of the first `StrokePath` in *page* coords: the path's anchor
/// points pushed through the command's transform. For the flat-rect emit
/// the path is the interned unit rect and the rectangle geometry +
/// stroke-alignment inset live in the transform, so the page-space
/// bounds are what reflect the alignment shift.
fn stroke_path_bounds(bytes: &[u8]) -> (f32, f32, f32, f32) {
    let document = idml_import::import_idml_doc(bytes).unwrap();
    let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
    let page = &built.pages[0];
    let (path_id, transform) = page
        .list
        .commands
        .iter()
        .find_map(|c| match c {
            DisplayCommand::StrokePath {
                path_id, transform, ..
            } => Some((*path_id, *transform)),
            _ => None,
        })
        .expect("a StrokePath");
    let path = page.list.paths.get(path_id).expect("path data");
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    let mut acc = |x: f32, y: f32| {
        let (px, py) = transform.apply(x, y);
        min_x = min_x.min(px);
        min_y = min_y.min(py);
        max_x = max_x.max(px);
        max_y = max_y.max(py);
    };
    for seg in &path.segments {
        match *seg {
            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => acc(x, y),
            PathSegment::QuadTo { x, y, .. } => acc(x, y),
            PathSegment::CubicTo { x, y, .. } => acc(x, y),
            PathSegment::Close => {}
        }
    }
    (min_x, min_y, max_x, max_y)
}

#[test]
fn striped_stroke_emits_one_strokepath_per_stripe() {
    let style = r#"<StripedStrokeStyle Self="StrokeStyle/Striped" Name="ThickThin">
        <Stripe Left="0" Width="0.6"/>
        <Stripe Left="0.8" Width="0.2"/>
      </StripedStrokeStyle>"#;
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="14" StrokeType="StrokeStyle/Striped""#,
        style,
    );
    let cmds = built_commands(&bytes);
    // A closed rect is one contour → one StrokePath per stripe (2).
    assert_eq!(
        stroke_paths(&cmds).len(),
        2,
        "two stripes → two StrokePath commands; got {:?}",
        cmds
    );
}

#[test]
fn striped_stroke_substroke_weights_match_stripe_fractions() {
    let style = r#"<StripedStrokeStyle Self="StrokeStyle/Striped" Name="ThickThin">
        <Stripe Left="0" Width="0.6"/>
        <Stripe Left="0.8" Width="0.2"/>
      </StripedStrokeStyle>"#;
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="10" StrokeType="StrokeStyle/Striped""#,
        style,
    );
    let cmds = built_commands(&bytes);
    let widths: Vec<f32> = stroke_paths(&cmds)
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::StrokePath { stroke, .. } => Some(stroke.width),
            _ => None,
        })
        .collect();
    // 0.6 × 10 = 6.0 and 0.2 × 10 = 2.0.
    assert!(widths.iter().any(|w| (w - 6.0).abs() < 1e-3), "{widths:?}");
    assert!(widths.iter().any(|w| (w - 2.0).abs() < 1e-3), "{widths:?}");
}

#[test]
fn wavy_stroke_emits_a_polyline_strokepath() {
    let style =
        r#"<WavyStrokeStyle Self="StrokeStyle/Wavy" Name="Wave" Width="0.5" Wavelength="2"/>"#;
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="10" StrokeType="StrokeStyle/Wavy""#,
        style,
    );
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
    let page = &built.pages[0];
    let strokes = stroke_paths(&page.list.commands);
    assert_eq!(strokes.len(), 1, "one wavy ribbon StrokePath");
    // The wavy path is a dense polyline (many LineTo), not the 4-corner
    // rectangle outline.
    let path_id = match strokes[0] {
        DisplayCommand::StrokePath { path_id, .. } => *path_id,
        _ => unreachable!(),
    };
    let segs = &page.list.paths.get(path_id).unwrap().segments;
    let line_tos = segs
        .iter()
        .filter(|s| matches!(s, PathSegment::LineTo { .. }))
        .count();
    assert!(
        line_tos > 8,
        "wavy ribbon should be densely sampled: {line_tos} LineTo"
    );
}

#[test]
fn gap_color_dash_emits_under_stroke_plus_dash() {
    let style = r#"<DashedStrokeStyle Self="StrokeStyle/GapDash" Name="GapDash"
        GapColor="Color/Cyan" GapTint="100" Pattern="8 6"/>"#;
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="8" StrokeType="StrokeStyle/GapDash""#,
        style,
    );
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
    let page = &built.pages[0];
    let strokes = stroke_paths(&page.list.commands);
    // Two StrokePaths: the gap-colour under-stroke (solid, full weight)
    // emitted first, then the dashed black stroke.
    assert_eq!(
        strokes.len(),
        2,
        "gap under-stroke + dash; got {:?}",
        page.list.commands
    );
    let (under, top) = match (strokes[0], strokes[1]) {
        (
            DisplayCommand::StrokePath { stroke: u, .. },
            DisplayCommand::StrokePath { stroke: t, .. },
        ) => (u, t),
        _ => unreachable!(),
    };
    // Under-stroke is solid full-weight; top stroke carries the dash.
    assert!(under.dash.is_solid(), "under-stroke is solid");
    assert!((under.width - 8.0).abs() < 1e-3, "under-stroke full weight");
    assert!(!top.dash.is_solid(), "top stroke is dashed");
}

/// Punch-list: the gap-colour pass must apply the style def's `GapTint`,
/// diluting the gap colour toward paper. Pre-fix the under-stroke painted
/// the gap colour at full strength regardless of `GapTint`. A tint < 100
/// must produce a lighter gap paint than tint = 100.
#[test]
fn gap_tint_lightens_the_rendered_gap_colour() {
    // Helper: the RGB of the first (under-)StrokePath's paint for a dashed
    // style whose gap is Cyan at the given tint.
    fn gap_under_rgb(tint: u32) -> paged_compose::Color {
        let style = format!(
            r#"<DashedStrokeStyle Self="StrokeStyle/GapDash" Name="GapDash"
                GapColor="Color/Cyan" GapTint="{tint}" Pattern="8 6"/>"#
        );
        let bytes = build_idml(
            r#"StrokeColor="Color/Black" StrokeWeight="8" StrokeType="StrokeStyle/GapDash""#,
            &style,
        );
        let document = idml_import::import_idml_doc(&bytes).unwrap();
        let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
        let page = &built.pages[0];
        let strokes = stroke_paths(&page.list.commands);
        // The gap under-stroke is drawn first.
        match strokes[0] {
            DisplayCommand::StrokePath { paint, .. } => match *paint {
                paged_compose::Paint::Solid(c) => c,
                paged_compose::Paint::Cmyk { rgb, .. } => rgb,
                other => panic!("unexpected gap paint {other:?}"),
            },
            _ => unreachable!(),
        }
    }

    let full = gap_under_rgb(100);
    let diluted = gap_under_rgb(40);

    // Cyan (CMYK 100 0 0 0) has a low red channel at full strength; tinting
    // toward paper white raises every channel toward 1.0. The red channel
    // is the discriminator (cyan's lowest), so a 40% tint must lift it.
    assert!(
        diluted.r > full.r + 0.2,
        "GapTint=40 must lighten the gap colour toward paper: \
         diluted r={} vs full r={}",
        diluted.r,
        full.r,
    );
    // Sanity: the diluted gap is genuinely lighter overall (higher sum).
    let lum = |c: paged_compose::Color| c.r + c.g + c.b;
    assert!(
        lum(diluted) > lum(full) + 0.3,
        "GapTint=40 must be lighter overall: diluted={:?} full={:?}",
        diluted,
        full,
    );
}

#[test]
fn frame_gap_color_overrides_style_def_gap_color() {
    // FINDING #7.5 — a per-FRAME `GapColor` (W0.3's mutation target)
    // wins over the `StrokeStyleDef`'s gap colour (W1.2). The style def
    // declares Cyan gap; the frame overrides with Black. The under-
    // stroke (drawn first) must carry the frame's Black, not Cyan.
    let style = r#"<DashedStrokeStyle Self="StrokeStyle/GapDash" Name="GapDash"
        GapColor="Color/Cyan" GapTint="100" Pattern="8 6"/>"#;
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="8" StrokeType="StrokeStyle/GapDash"
           GapColor="Color/Black" GapTint="100""#,
        style,
    );
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
    let page = &built.pages[0];
    let strokes = stroke_paths(&page.list.commands);
    assert_eq!(
        strokes.len(),
        2,
        "gap under-stroke + dash; got {:?}",
        page.list.commands
    );
    // The under-stroke (drawn first) carries the gap paint. The frame's
    // Black (CMYK 0 0 0 100 → near-black RGB) must win over the style's
    // Cyan (CMYK 100 0 0 0 → cyan RGB).
    let under_paint = match strokes[0] {
        DisplayCommand::StrokePath { paint, .. } => *paint,
        _ => unreachable!(),
    };
    let c = match under_paint {
        paged_compose::Paint::Solid(c) => c,
        paged_compose::Paint::Cmyk { rgb, .. } => rgb,
        other => panic!("unexpected gap paint {other:?}"),
    };
    // Black gap: all channels low. Cyan would have a high blue/green.
    assert!(
        c.r < 0.3 && c.g < 0.3 && c.b < 0.3,
        "frame GapColor=Black must win over style Cyan, got {c:?}"
    );
}

#[test]
fn frame_gap_color_paints_when_style_def_has_none() {
    // FINDING #7.5 — a dashed frame with NO style-def gap colour but a
    // per-frame `GapColor` still paints the gap under-stroke (pre-fix the
    // `Plain { gap_color: None }` class made the stroke non-styled and
    // the frame override was dropped → zero pixel delta in the editor).
    let style = r#"<DashedStrokeStyle Self="StrokeStyle/PlainDash" Name="PlainDash"
        Pattern="8 6"/>"#;
    let no_gap = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="8" StrokeType="StrokeStyle/PlainDash""#,
        style,
    );
    let with_gap = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="8" StrokeType="StrokeStyle/PlainDash"
           GapColor="Color/Cyan" GapTint="100""#,
        style,
    );
    let n_no = stroke_paths(&built_commands(&no_gap)).len();
    let n_with = stroke_paths(&built_commands(&with_gap)).len();
    assert!(
        n_with > n_no,
        "per-frame GapColor must add the gap under-stroke: no-gap={n_no}, with-gap={n_with}"
    );
}

#[test]
fn frame_dash_array_override_paints_and_wins_over_style_pattern() {
    // W1.1 — a per-FRAME `StrokeDashAndGap` override (the new mutation
    // target) must paint, AND win over the `StrokeStyleDef` pattern the
    // frame's `StrokeType` references. The style declares dashes of
    // [3 2]; the frame overrides with [11 4]. The emitted StrokePath
    // must carry the FRAME's [11 4], proving the instance dash paints
    // with precedence (the gap-colour FINDING #7.5 precedent).
    let style = r#"<DashedStrokeStyle Self="StrokeStyle/StyleDash" Name="StyleDash"
        Pattern="3 2"/>"#;
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="6" StrokeType="StrokeStyle/StyleDash"
           StrokeDashAndGap="11 4""#,
        style,
    );
    let cmds = built_commands(&bytes);
    let strokes = stroke_paths(&cmds);
    assert_eq!(strokes.len(), 1, "one dashed StrokePath; got {cmds:?}");
    let dash = match strokes[0] {
        DisplayCommand::StrokePath { stroke, .. } => stroke.dash,
        _ => unreachable!(),
    };
    assert!(!dash.is_solid(), "instance dash must paint");
    assert_eq!(
        dash.as_slice(),
        &[11.0, 4.0],
        "frame StrokeDashAndGap must win over the style def's [3 2]"
    );
}

#[test]
fn empty_frame_dash_array_falls_back_to_style_pattern() {
    // W1.1 — clearing the per-frame override (no `StrokeDashAndGap`
    // attribute) falls back to the `StrokeType`'s style pattern, so the
    // dash is still the style's [3 2] (the empty-clears contract on the
    // paint side).
    let style = r#"<DashedStrokeStyle Self="StrokeStyle/StyleDash" Name="StyleDash"
        Pattern="3 2"/>"#;
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="6" StrokeType="StrokeStyle/StyleDash""#,
        style,
    );
    let cmds = built_commands(&bytes);
    let dash = stroke_paths(&cmds)
        .iter()
        .find_map(|c| match c {
            DisplayCommand::StrokePath { stroke, .. } => Some(stroke.dash),
            _ => None,
        })
        .expect("a dashed StrokePath");
    assert_eq!(dash.as_slice(), &[3.0, 2.0], "falls back to style [3 2]");
}

#[test]
fn solid_stroke_without_style_is_a_single_strokepath() {
    // Sanity floor: a plain solid stroke with no custom style still
    // emits exactly one StrokePath (no gap pass, no stripes).
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="6" StrokeType="StrokeStyle/$ID/Solid""#,
        "",
    );
    let cmds = built_commands(&bytes);
    assert_eq!(stroke_paths(&cmds).len(), 1);
}

#[test]
fn stroke_alignment_inside_shifts_bounds_inward() {
    // Inside alignment insets the polygon-stroke path by weight/2. The
    // base rect outline is x∈[0,200], y∈[0,100]; weight 20 ⇒ inset 10.
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeType="StrokeStyle/$ID/Solid" StrokeAlignment="InsideAlignment""#,
        "",
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    // Rectangle path (flat-rect emit) insets to [10,190]×[10,90].
    assert!((min_x - 10.0).abs() < 1e-3, "min_x={min_x}");
    assert!((min_y - 10.0).abs() < 1e-3, "min_y={min_y}");
    assert!((max_x - 190.0).abs() < 1e-3, "max_x={max_x}");
    assert!((max_y - 90.0).abs() < 1e-3, "max_y={max_y}");
}

#[test]
fn stroke_alignment_outside_shifts_bounds_outward() {
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeType="StrokeStyle/$ID/Solid" StrokeAlignment="OutsideAlignment""#,
        "",
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    // Outset to [-10,210]×[-10,110].
    assert!((min_x + 10.0).abs() < 1e-3, "min_x={min_x}");
    assert!((min_y + 10.0).abs() < 1e-3, "min_y={min_y}");
    assert!((max_x - 210.0).abs() < 1e-3, "max_x={max_x}");
    assert!((max_y - 110.0).abs() < 1e-3, "max_y={max_y}");
}

#[test]
fn stroke_alignment_center_keeps_bounds() {
    let bytes = build_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeType="StrokeStyle/$ID/Solid" StrokeAlignment="CenterAlignment""#,
        "",
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    assert!(
        min_x.abs() < 1e-3 && min_y.abs() < 1e-3,
        "min=({min_x},{min_y})"
    );
    assert!(
        (max_x - 200.0).abs() < 1e-3 && (max_y - 100.0).abs() < 1e-3,
        "max=({max_x},{max_y})"
    );
}

// ----------------------------------------------------------------------
// W1.5 — stroke alignment on closed NON-rect shapes (oval + polygon).
//
// The renderer offsets the flattened closed outline inward (Inside) /
// outward (Outside) by weight/2, then strokes the offset path. We assert
// the page-space bounds of the emitted `StrokePath` shift the right way:
// inside shrinks the outline, outside grows it, centre leaves it.
// ----------------------------------------------------------------------

/// Single-page IDML carrying one `<Oval>` with a 200×100 GeometricBounds
/// at inner origin, plus the supplied stroke attributes.
fn build_oval_idml(stroke_attrs: &str) -> Vec<u8> {
    build_shape_idml("Oval", "", stroke_attrs)
}

/// Single-page IDML carrying one `<Polygon>` — a 200×100 axis-aligned
/// quad declared via PathGeometry — plus the supplied stroke attributes.
fn build_polygon_idml(stroke_attrs: &str) -> Vec<u8> {
    let geom = r#"<Properties>
          <PathGeometry>
            <GeometryPathType PathOpen="false">
              <PathPointArray>
                <PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/>
                <PathPointType Anchor="200 0" LeftDirection="200 0" RightDirection="200 0"/>
                <PathPointType Anchor="200 100" LeftDirection="200 100" RightDirection="200 100"/>
                <PathPointType Anchor="0 100" LeftDirection="0 100" RightDirection="0 100"/>
              </PathPointArray>
            </GeometryPathType>
          </PathGeometry>
        </Properties>"#;
    build_shape_idml("Polygon", geom, stroke_attrs)
}

/// Build a single-page IDML with one shape element of `tag` (Oval /
/// Polygon) carrying `GeometricBounds="0 0 100 200"`, the supplied inner
/// `body` (e.g. a `<Properties><PathGeometry>…`), and `stroke_attrs`.
fn build_shape_idml(tag: &str, body: &str, stroke_attrs: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Graphic src="Resources/Graphic.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Black" Name="Black" Space="CMYK" ColorValue="0 0 0 100"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 300"/>
    <{tag} Self="s1" GeometricBounds="0 0 100 200" {stroke_attrs}>
      {body}
    </{tag}>
  </Spread>
</idPkg:Spread>"#
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn oval_stroke_alignment_inside_shifts_bounds_inward() {
    // The oval outline spans x∈[0,200], y∈[0,100]; weight 20 ⇒ the
    // Inside-aligned outline insets by 10 on every side. The emitted
    // StrokePath's page-space bounds therefore shrink to ~[10,190]×[10,90].
    let bytes = build_oval_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeAlignment="InsideAlignment""#,
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    assert!(min_x > 8.0 && min_x < 12.0, "inset min_x={min_x}");
    assert!(min_y > 8.0 && min_y < 12.0, "inset min_y={min_y}");
    assert!(max_x > 188.0 && max_x < 192.0, "inset max_x={max_x}");
    assert!(max_y > 88.0 && max_y < 92.0, "inset max_y={max_y}");
}

#[test]
fn oval_stroke_alignment_outside_shifts_bounds_outward() {
    let bytes = build_oval_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeAlignment="OutsideAlignment""#,
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    assert!(min_x > -12.0 && min_x < -8.0, "outset min_x={min_x}");
    assert!(min_y > -12.0 && min_y < -8.0, "outset min_y={min_y}");
    assert!(max_x > 208.0 && max_x < 212.0, "outset max_x={max_x}");
    assert!(max_y > 108.0 && max_y < 112.0, "outset max_y={max_y}");
}

#[test]
fn oval_stroke_alignment_center_keeps_outline() {
    // Centre alignment keeps the natural ellipse primitive — no offset
    // StrokePath. The emitted command is the centred ellipse stroke
    // (a StrokePath against the unit ellipse via the rect transform);
    // its page-space bounds equal the GeometricBounds.
    let bytes = build_oval_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeAlignment="CenterAlignment""#,
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    assert!(
        min_x.abs() < 1.0 && min_y.abs() < 1.0,
        "min=({min_x},{min_y})"
    );
    assert!(
        (max_x - 200.0).abs() < 1.0 && (max_y - 100.0).abs() < 1.0,
        "max=({max_x},{max_y})"
    );
}

#[test]
fn polygon_stroke_alignment_inside_shifts_bounds_inward() {
    let bytes = build_polygon_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeAlignment="InsideAlignment""#,
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    // Inset by weight/2 = 10 on every side.
    assert!((min_x - 10.0).abs() < 1e-2, "inset min_x={min_x}");
    assert!((min_y - 10.0).abs() < 1e-2, "inset min_y={min_y}");
    assert!((max_x - 190.0).abs() < 1e-2, "inset max_x={max_x}");
    assert!((max_y - 90.0).abs() < 1e-2, "inset max_y={max_y}");
}

#[test]
fn polygon_stroke_alignment_outside_shifts_bounds_outward() {
    let bytes = build_polygon_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeAlignment="OutsideAlignment""#,
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    // Outset by 10 on every side.
    assert!((min_x + 10.0).abs() < 1e-2, "outset min_x={min_x}");
    assert!((min_y + 10.0).abs() < 1e-2, "outset min_y={min_y}");
    assert!((max_x - 210.0).abs() < 1e-2, "outset max_x={max_x}");
    assert!((max_y - 110.0).abs() < 1e-2, "outset max_y={max_y}");
}

#[test]
fn polygon_stroke_alignment_center_keeps_outline() {
    let bytes = build_polygon_idml(
        r#"StrokeColor="Color/Black" StrokeWeight="20" StrokeAlignment="CenterAlignment""#,
    );
    let (min_x, min_y, max_x, max_y) = stroke_path_bounds(&bytes);
    assert!(
        min_x.abs() < 1e-2 && min_y.abs() < 1e-2,
        "min=({min_x},{min_y})"
    );
    assert!(
        (max_x - 200.0).abs() < 1e-2 && (max_y - 100.0).abs() < 1e-2,
        "max=({max_x},{max_y})"
    );
}

// ----------------------------------------------------------------------
// Punch-list (rides v35) — `frameStrokeMiterLimit` on closed polygons,
// not just rectangles. The renderer's stroke-join code already bevels a
// miter past the limit (tiny-skia / Vello stroker); the wire just had to
// thread the polygon's `MiterLimit` attribute through to the display-list
// `Stroke`. The first test asserts that wire; the second proves the limit
// changes the rendered acute apex (high limit → mitered spike, low limit
// → bevelled flat).
// ----------------------------------------------------------------------

/// Single-page IDML carrying one sharp-apex `<Polygon>` triangle (apex
/// at the top, near `(100,10)`) plus the supplied stroke attributes. The
/// acute apex is where the miter limit decides spike-vs-bevel.
fn build_spike_polygon_idml(stroke_attrs: &str) -> Vec<u8> {
    let geom = r#"<Properties>
          <PathGeometry>
            <GeometryPathType PathOpen="false">
              <PathPointArray>
                <PathPointType Anchor="100 10" LeftDirection="100 10" RightDirection="100 10"/>
                <PathPointType Anchor="120 150" LeftDirection="120 150" RightDirection="120 150"/>
                <PathPointType Anchor="80 150" LeftDirection="80 150" RightDirection="80 150"/>
              </PathPointArray>
            </GeometryPathType>
          </PathGeometry>
        </Properties>"#;
    build_shape_idml("Polygon", geom, stroke_attrs)
}

/// `Stroke.miter_limit` of the first emitted `StrokePath`.
fn first_stroke_miter_limit(bytes: &[u8]) -> f32 {
    let cmds = built_commands(bytes);
    cmds.iter()
        .find_map(|c| match c {
            DisplayCommand::StrokePath { stroke, .. } => Some(stroke.miter_limit),
            _ => None,
        })
        .expect("a StrokePath")
}

#[test]
fn polygon_miter_limit_threads_into_display_list_stroke() {
    // A polygon's `MiterLimit` attribute now reaches the display-list
    // `Stroke` (previously Rectangle-only; non-rect kinds defaulted to
    // the 4.0 PDF default no matter what the IDML declared).
    let bytes =
        build_polygon_idml(r#"StrokeColor="Color/Black" StrokeWeight="8" MiterLimit="1.5""#);
    assert!(
        (first_stroke_miter_limit(&bytes) - 1.5).abs() < 1e-4,
        "polygon MiterLimit=1.5 must thread through; got {}",
        first_stroke_miter_limit(&bytes)
    );

    // And a high declared limit threads through too (control).
    let bytes_hi =
        build_polygon_idml(r#"StrokeColor="Color/Black" StrokeWeight="8" MiterLimit="20""#);
    assert!(
        (first_stroke_miter_limit(&bytes_hi) - 20.0).abs() < 1e-4,
        "polygon MiterLimit=20 must thread through; got {}",
        first_stroke_miter_limit(&bytes_hi)
    );
}

#[test]
fn polygon_sharp_corner_bevels_past_miter_limit() {
    use paged_compose::Color;

    // Render the spike at a HIGH miter limit (the acute apex extends into
    // a long mitered point that pokes above the geometric apex) and at a
    // LOW limit (the join bevels flat, so the region above the apex stays
    // background). Probe a pixel a few px ABOVE the apex tip at (100,10):
    // mitered → painted dark, bevelled → background white.
    let render_apex = |miter: &str| -> [u8; 4] {
        let attrs = format!(
            r#"StrokeColor="Color/Black" StrokeWeight="22" EndJoin="MiterEndJoin" MiterLimit="{miter}""#
        );
        let bytes = build_spike_polygon_idml(&attrs);
        let document = idml_import::import_idml_doc(&bytes).unwrap();
        let opts = PipelineOptions::default();
        let (_built, images) =
            pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();
        // Page 300×400 at 72 dpi → 300×400 px (1pt ≈ 1px). Apex tip at
        // inner/page (100,10); probe (100,4), just above the tip.
        images[0].get_pixel(100, 4).0
    };

    let mitered = render_apex("20");
    let bevelled = render_apex("1");

    // High limit: the long miter point covers (100,4) → dark.
    assert!(
        mitered[0] < 120 && mitered[1] < 120 && mitered[2] < 120,
        "high miter limit should paint the mitered spike above the apex; got {mitered:?}"
    );
    // Low limit: the join bevels back, leaving the region above the apex
    // as white background.
    assert!(
        bevelled[0] > 200 && bevelled[1] > 200 && bevelled[2] > 200,
        "low miter limit should bevel the apex, leaving background; got {bevelled:?}"
    );
    // The luminance gap is the load-bearing difference.
    assert!(
        bevelled[0] as i32 - mitered[0] as i32 > 100,
        "miter vs bevel must differ at the apex: bevelled {} vs mitered {}",
        bevelled[0],
        mitered[0]
    );
}
