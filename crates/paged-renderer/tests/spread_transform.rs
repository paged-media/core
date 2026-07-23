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

//! W1.9 — spread + master-spread `ItemTransform` rotation/scale.
//!
//! Builds a one-page IDML whose `<Spread>` carries a 90° rotation and
//! whose applied `<MasterSpread>` carries its own 90° rotation, each
//! holding a single rectangle. Asserts:
//!
//!   * the BODY rect's emitted `FillPath` transform carries the spread's
//!     rotation (the 2×2 linear block is the 90° matrix, not identity);
//!   * the MASTER rect's emitted `FillPath` transform carries the master
//!     spread's rotation (stamped through the master overlay chain);
//!   * with an identity spread transform the body rect's transform is
//!     the plain page-origin shift (regression floor — proves the common
//!     case stays byte-identical).

use paged_compose::{Color, DisplayCommand, Paint};
use paged_renderer::{pipeline, PipelineOptions};
use std::io::Write;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// Build an IDML: one body spread (`spread_xform` on `<Spread>`) holding
/// a black rect, applying a master spread (`master_xform` on
/// `<MasterSpread>`) that holds its own grey rect. Page is 400×400 so a
/// 90° rotation keeps it square (no AABB swap to reason about).
fn build_idml(spread_xform: &str, master_xform: &str) -> Vec<u8> {
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
  <idPkg:Graphic src="Resources/Graphic.xml"/>
  <idPkg:MasterSpread src="MasterSpreads/MasterSpread_m1.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Black" Name="Black" Space="CMYK" ColorValue="0 0 0 100"/>
    <Color Self="Color/Grey" Name="Grey" Space="CMYK" ColorValue="0 0 0 50"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    // Master spread: a grey rect at inner (40,40) sized 40×40.
    let master = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:MasterSpread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <MasterSpread Self="m1" ItemTransform="{master_xform}">
    <Page Self="mp1" GeometricBounds="0 0 400 400"/>
    <Rectangle Self="mr1" GeometricBounds="40 40 80 80" FillColor="Color/Grey" StrokeWeight="0">
      <Properties>
        <PathGeometry>
          <GeometryPathType PathOpen="false">
            <PathPointArray>
              <PathPointType Anchor="40 40" LeftDirection="40 40" RightDirection="40 40"/>
              <PathPointType Anchor="80 40" LeftDirection="80 40" RightDirection="80 40"/>
              <PathPointType Anchor="80 80" LeftDirection="80 80" RightDirection="80 80"/>
              <PathPointType Anchor="40 80" LeftDirection="40 80" RightDirection="40 80"/>
            </PathPointArray>
          </GeometryPathType>
        </PathGeometry>
      </Properties>
    </Rectangle>
  </MasterSpread>
</idPkg:MasterSpread>"#
    );
    zip.start_file("MasterSpreads/MasterSpread_m1.xml", deflated)
        .unwrap();
    zip.write_all(master.as_bytes()).unwrap();

    // Body spread: a black rect at inner (100,100) sized 50×50.
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" ItemTransform="{spread_xform}">
    <Page Self="p1" AppliedMaster="m1" GeometricBounds="0 0 400 400"/>
    <Rectangle Self="r1" GeometricBounds="100 100 150 150" FillColor="Color/Black" StrokeWeight="0">
      <Properties>
        <PathGeometry>
          <GeometryPathType PathOpen="false">
            <PathPointArray>
              <PathPointType Anchor="100 100" LeftDirection="100 100" RightDirection="100 100"/>
              <PathPointType Anchor="150 100" LeftDirection="150 100" RightDirection="150 100"/>
              <PathPointType Anchor="150 150" LeftDirection="150 150" RightDirection="150 150"/>
              <PathPointType Anchor="100 150" LeftDirection="100 150" RightDirection="100 150"/>
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

/// All `FillPath` transforms in document order, paired with their solid
/// paint colour (so we can pick the body/black vs master/grey rect).
fn fill_transforms(bytes: &[u8]) -> Vec<(Color, [f32; 6])> {
    let document = paged_parse::import_idml_doc(bytes).unwrap();
    let built = pipeline::build_document(&document, &PipelineOptions::default()).unwrap();
    let page = &built.pages[0];
    page.list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath {
                paint: Paint::Solid(col),
                transform,
                ..
            } => Some((*col, transform.0)),
            DisplayCommand::FillPath {
                paint: Paint::Cmyk { rgb, .. },
                transform,
                ..
            } => Some((*rgb, transform.0)),
            _ => None,
        })
        .collect()
}

// The two rects fill via the interned UNIT_RECT path, so each FillPath's
// transform is `outer ∘ scale(w, h)` — the rect dims are baked into the
// linear block. The body rect is 50×50, the master rect 40×40. Pure
// black (CMYK 0 0 0 100 → RGB ≈ 0) is the body; the 50%-grey master
// (CMYK 0 0 0 50 → RGB ≈ 0.21) is the master.
fn is_body(c: Color) -> bool {
    c.r < 0.05 && c.g < 0.05 && c.b < 0.05
}

fn is_master(c: Color) -> bool {
    c.r > 0.1 && c.r < 0.4 && (c.r - c.g).abs() < 0.05 && (c.r - c.b).abs() < 0.05
}

/// The 2×2 linear block of a 2×3 matrix.
fn linear(m: [f32; 6]) -> [f32; 4] {
    [m[0], m[1], m[2], m[3]]
}

/// Assert the linear block ≈ `expected`.
fn assert_linear(m: [f32; 6], expected: [f32; 4]) {
    let l = linear(m);
    for i in 0..4 {
        assert!(
            (l[i] - expected[i]).abs() < 1e-3,
            "linear[{i}]={} expected {} (full {l:?} vs {expected:?})",
            l[i],
            expected[i]
        );
    }
}

const ROT90: &str = "0 1 -1 0 0 0";
const IDENTITY: &str = "1 0 0 1 0 0";

#[test]
fn identity_spread_keeps_body_transform_unrotated() {
    // Regression floor: with no spread/master transform the body rect's
    // FillPath linear block is just the rect's own 50×50 scale (the
    // UNIT_RECT → rect mapping) — no rotation, byte-identical to pre-W1.9.
    let bytes = build_idml(IDENTITY, IDENTITY);
    let fills = fill_transforms(&bytes);
    let (_, body) = fills
        .iter()
        .find(|(c, _)| is_body(*c))
        .expect("a black body fill");
    assert_linear(*body, [50.0, 0.0, 0.0, 50.0]);
}

#[test]
fn rotated_body_spread_rotates_the_body_rect_transform() {
    // A 90° body spread rotation composes onto the body rect's
    // UNIT_RECT scale: rot90 · diag(50,50) = [0 50 -50 0]. Pre-W1.9 this
    // would have stayed [50 0 0 50].
    let bytes = build_idml(ROT90, IDENTITY);
    let fills = fill_transforms(&bytes);
    let (_, body) = fills
        .iter()
        .find(|(c, _)| is_body(*c))
        .expect("a black body fill");
    assert_linear(*body, [0.0, 50.0, -50.0, 0.0]);
}

#[test]
fn rotated_master_spread_rotates_the_master_rect_transform() {
    // The master overlay rect must pick up the MASTER spread's own 90°
    // rotation through the master-stamp chain, even when the BODY spread
    // is unrotated: rot90 · diag(40,40) = [0 40 -40 0].
    let bytes = build_idml(IDENTITY, ROT90);
    let fills = fill_transforms(&bytes);
    let (_, master) = fills
        .iter()
        .find(|(c, _)| is_master(*c))
        .expect("a grey master fill");
    assert_linear(*master, [0.0, 40.0, -40.0, 0.0]);
}

#[test]
fn unrotated_master_keeps_master_rect_unrotated() {
    // Floor for the master path: an identity master spread leaves the
    // master rect's linear block at its plain 40×40 scale.
    let bytes = build_idml(IDENTITY, IDENTITY);
    let fills = fill_transforms(&bytes);
    let (_, master) = fills
        .iter()
        .find(|(c, _)| is_master(*c))
        .expect("a grey master fill");
    assert_linear(*master, [40.0, 0.0, 0.0, 40.0]);
}

#[test]
fn rotated_master_and_body_both_rotate() {
    // Both spreads rotated: the body rect rides the body spread's
    // rotation; the master rect rides the master spread's rotation
    // composed with the body page's rotation (the overlay is stamped
    // onto the rotated body page → 180° net = [-40 0 0 -40]).
    let bytes = build_idml(ROT90, ROT90);
    let fills = fill_transforms(&bytes);
    let body = fills.iter().find(|(c, _)| is_body(*c)).map(|(_, m)| *m);
    let master = fills.iter().find(|(c, _)| is_master(*c)).map(|(_, m)| *m);
    let body = body.expect("body fill");
    let master = master.expect("master fill");
    // Body: rot90 · diag(50,50).
    assert_linear(body, [0.0, 50.0, -50.0, 0.0]);
    // Master: body-rot90 · master-rot90 · diag(40,40) = rot180 · diag = -diag.
    assert_linear(master, [-40.0, 0.0, 0.0, -40.0]);
}
