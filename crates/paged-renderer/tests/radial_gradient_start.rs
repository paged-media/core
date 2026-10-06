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

//! v70 — a radial gradient with `GradientFillStart` + `GradientFillLength`
//! is centred at the start point with the length as its radius, for a
//! rectangle, an oval and a polygon. Before, the start was not read and
//! every radial sat at InDesign's swatch default (the bottom-left corner),
//! so PowerPoint's centred glows rendered off-centre.

use std::io::Write;

use paged_compose::Color;
use paged_renderer::pipeline::{self, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// A 400 × 300 page with one item filled Sun → Sky radially. The item
/// spans x 100..400, y 50..250; the gradient starts at (160, 150) with a
/// 40 pt radius.
fn idml(item: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", o).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", o).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Graphic src="Resources/Graphic.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", o).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Sun" Name="Sun" Space="RGB" ColorValue="255 200 80"/>
    <Color Self="Color/Sky" Name="Sky" Space="RGB" ColorValue="60 120 220"/>
    <Gradient Self="Gradient/Glow" Name="Glow" Type="Radial">
      <GradientStop StopColor="Color/Sun" Location="0"/>
      <GradientStop StopColor="Color/Sky" Location="100"/>
    </Gradient>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", o).unwrap();
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 300 400"/>
    {item}
  </Spread>
</idPkg:Spread>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

const PLACEMENT: &str = r#"FillColor="Gradient/Glow" StrokeWeight="0" GradientFillStart="160 150" GradientFillLength="40""#;

fn assert_centred(item: &str, what: &str, corner: bool) {
    let doc = idml_import::import_idml_doc(&idml(item)).expect("import");
    let (_, images) =
        pipeline::render_document(&doc, &PipelineOptions::default(), 72.0, Color::WHITE)
            .expect("render");
    let img = &images[0];
    let warm = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        i32::from(p[0]) - i32::from(p[2])
    };
    // At the start point: the first stop.
    assert!(
        warm(160, 150) > 150,
        "{what}: centre {:?}",
        img.get_pixel(160, 150)
    );
    // Half the radius out, either side: still more Sun than Sky.
    assert!(
        warm(140, 150) > 0 && warm(180, 150) > 0,
        "{what}: inside the radius"
    );
    // Beyond the radius, in every direction: the last stop.
    let mut far = vec![(220, 150), (110, 150), (160, 210), (160, 90)];
    if corner {
        far.push((390, 240));
    }
    for (x, y) in far {
        assert!(
            warm(x, y) < -120,
            "{what}: ({x}, {y}) {:?}",
            img.get_pixel(x, y)
        );
    }
}

#[test]
fn a_rectangles_radial_is_centred_at_its_start() {
    assert_centred(
        &format!(r#"<Rectangle Self="r1" GeometricBounds="50 100 250 400" {PLACEMENT}/>"#),
        "rectangle",
        true,
    );
}

#[test]
fn an_ovals_radial_is_centred_at_its_start() {
    // The far corner lies outside the ellipse, so it is not probed.
    assert_centred(
        &format!(r#"<Oval Self="o1" GeometricBounds="50 60 250 400" {PLACEMENT}/>"#),
        "oval",
        false,
    );
}

#[test]
fn a_polygons_radial_is_centred_at_its_start() {
    let pts = [(100, 50), (400, 50), (400, 250), (100, 250)]
        .iter()
        .map(|(x, y)| {
            format!(r#"<PathPointType Anchor="{x} {y}" LeftDirection="{x} {y}" RightDirection="{x} {y}"/>"#)
        })
        .collect::<String>();
    assert_centred(
        &format!(
            r#"<Polygon Self="g1" ItemTransform="1 0 0 1 0 0" {PLACEMENT}><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>{pts}</PathPointArray></GeometryPathType></PathGeometry></Properties></Polygon>"#
        ),
        "polygon",
        true,
    );
}
