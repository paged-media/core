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

//! Master items paint in the master spread's stacking order, and a
//! master's placed picture paints on every page that applies it.
//!
//! A slide master typically carries a full-page background shape with a
//! placed photo above it. The master pass used to walk the backing vecs
//! kind by kind (every rectangle, then every polygon), so the
//! background polygon covered the photo; and it never drew placed
//! images at all, because the image caches were set up after it.

use std::io::Write;

use paged_compose::Color;
use paged_renderer::pipeline::{self, PipelineOptions};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// The rectangle `x0 y0 x1 y1` as IDML path points.
fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> String {
    let pt = |x: f32, y: f32| {
        format!(
            r#"<PathPointType Anchor="{x} {y}" LeftDirection="{x} {y}" RightDirection="{x} {y}"/>"#
        )
    };
    format!(
        r#"<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>{}{}{}{}</PathPointArray></GeometryPathType></PathGeometry></Properties>"#,
        pt(x0, y0),
        pt(x1, y0),
        pt(x1, y1),
        pt(x0, y1)
    )
}

fn red_pixel_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([230, 0, 0, 255]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// Master: full-page blue polygon (bottom), a picture frame over the
/// middle, a small green polygon on top of the picture's centre. The body
/// page applies the master and carries nothing of its own.
fn build() -> Vec<u8> {
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
  <Color Self="Color/Blue" Model="Process" Space="RGB" ColorValue="0 0 255" Name="Blue"/>
  <Color Self="Color/Green" Model="Process" Space="RGB" ColorValue="0 200 0" Name="Green"/>
</idPkg:Graphic>"#,
    )
    .unwrap();
    let master = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:MasterSpread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <MasterSpread Self="m1" Name="A-Master" ItemTransform="1 0 0 1 0 0">
    <Page Self="m1pg" GeometricBounds="0 0 200 200" ItemTransform="1 0 0 1 0 0"/>
    <Polygon Self="bg" FillColor="Color/Blue" StrokeWeight="0" ItemTransform="1 0 0 1 0 0">{bg}</Polygon>
    <Rectangle Self="pic" StrokeWeight="0" ContentType="GraphicType" ItemTransform="1 0 0 1 0 0">{pic}
      <Image Self="picI" LinkResourceURI="photo.png" ItemTransform="120 0 0 120 40 40"/>
    </Rectangle>
    <Polygon Self="dot" FillColor="Color/Green" StrokeWeight="0" ItemTransform="1 0 0 1 0 0">{dot}</Polygon>
  </MasterSpread>
</idPkg:MasterSpread>"#,
        bg = square(0.0, 0.0, 200.0, 200.0),
        pic = square(40.0, 40.0, 160.0, 160.0),
        dot = square(90.0, 90.0, 110.0, 110.0),
    );
    zip.start_file("MasterSpreads/MasterSpread_m1.xml", deflated)
        .unwrap();
    zip.write_all(master.as_bytes()).unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" ItemTransform="1 0 0 1 0 0">
    <Page Self="p1" AppliedMaster="m1" GeometricBounds="0 0 200 200" ItemTransform="1 0 0 1 0 0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn master_items_paint_in_stacking_order_with_their_pictures() {
    let document = idml_import::import_idml_doc(&build()).unwrap();
    let mut br = paged_renderer::BytesResolver::new();
    br.add_image("photo.png", red_pixel_png());
    let opts = PipelineOptions {
        assets: Some(&br),
        ..PipelineOptions::default()
    };
    let (_, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();
    let img = &images[0];
    let px = |x, y| img.get_pixel(x, y).0;

    let corner = px(10, 10);
    assert!(
        corner[2] > 200 && corner[0] < 40,
        "master background polygon paints at the corner, got {corner:?}"
    );
    let picture = px(60, 60);
    assert!(
        picture[0] > 200 && picture[1] < 40 && picture[2] < 40,
        "master picture paints above the background polygon, got {picture:?}"
    );
    let dot = px(100, 100);
    assert!(
        dot[1] > 150 && dot[0] < 40,
        "the polygon above the picture stays above it, got {dot:?}"
    );
}
