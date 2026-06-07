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

//! W1.9 — renderer / hit-test PARITY under a spread `ItemTransform`
//! rotation (the cycle-8 lesson: a transform the painter applies but the
//! hit-tester ignores breaks selection silently).
//!
//! The renderer rotates a page's content about the page origin via the
//! page's `spread_transform`; the hit-tester inverts the SAME field. So a
//! doc-point that — mapped back through the spread rotation — lands inside
//! a frame's inner bounds must resolve to that frame, and one that lands
//! outside must not. We assert both for a 90° body-spread rotation, and
//! that the identity case still hits the un-rotated position.

use std::io::Write;

use paged_canvas::{CanvasModel, CanvasOptions, PageId};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One-page IDML, body `<Spread ItemTransform=spread_xform>`, holding a
/// single 50×50 rect at inner (100,100)-(150,150).
fn build_idml(spread_xform: &str) -> Vec<u8> {
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
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" ItemTransform="{spread_xform}">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
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

const ROT90: &str = "0 1 -1 0 0 0";
const IDENTITY: &str = "1 0 0 1 0 0";

fn model(spread_xform: &str) -> CanvasModel {
    let bytes = build_idml(spread_xform);
    CanvasModel::load("doc", &bytes, CanvasOptions::default()).expect("load + build")
}

#[test]
fn identity_spread_hits_the_unrotated_rect() {
    // No spread transform: the rect lives at page-local (100..150)².
    // A click at its centre (125,125) resolves to it; a click well
    // outside does not.
    let m = model(IDENTITY);
    let page = PageId("p1".to_string());
    let hit = m.hit_test(&page, (125.0, 125.0));
    assert!(hit.element.is_some(), "centre of un-rotated rect must hit");
    let miss = m.hit_test(&page, (300.0, 300.0));
    assert!(miss.element.is_none(), "far point must miss");
}

#[test]
fn rotated_spread_hits_the_rotated_rect_position() {
    // 90° spread rotation about the page origin. The renderer paints the
    // rect's inner (125,125) centre at page-local S·(125,125) =
    // (-125, 125) (x' = -y, y' = x). The hit-tester inverts the SAME
    // spread_transform, so a click at (-125, 125) must resolve to the
    // rect — proving painter / hit-test agree.
    let m = model(ROT90);
    let page = PageId("p1".to_string());
    let hit = m.hit_test(&page, (-125.0, 125.0));
    assert!(
        hit.element.is_some(),
        "rotated rect centre (-125,125) must hit under the rotated spread"
    );
    // The UN-rotated centre (125,125) now lands OUTSIDE the rotated rect
    // (its inverse maps to (125,-125), off the [100,150]² inner box) — a
    // pre-W1.9 hit-tester that ignored the spread transform would wrongly
    // hit here, so this guards the divergence the cycle-8 lesson warns of.
    let miss = m.hit_test(&page, (125.0, 125.0));
    assert!(
        miss.element.is_none(),
        "the un-rotated centre must MISS once the spread is rotated (parity)"
    );
}
