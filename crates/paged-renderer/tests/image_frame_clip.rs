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

//! A placed picture with no inner transform of its own is clipped to its
//! frame and drawn the way the same frame with that placement spelled as
//! a transform is drawn.
//!
//! InDesign clips a frame's content to the frame. The engine drew a
//! picture that carried no `<Image ItemTransform>` and no crop-less
//! named fit as a bare rect over the frame box minus its crops: a
//! negative crop's overhang painted outside the frame, and the frame's
//! fractional edges came out as opaque pixels where the clipped path —
//! the one an IDML export of the same frame takes, since the export
//! spells the placement as a transform — covers them partly.

use std::io::Write;

use paged_compose::Color;
use paged_renderer::{pipeline, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// A one-spread package whose only item is `rect` (a `<Rectangle>`
/// element, written verbatim).
fn package(rect: &str) -> Vec<u8> {
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
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    {rect}
  </Spread>
</idPkg:Spread>"#
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

fn render(idml: &[u8], picture: Vec<u8>, dpi: f32) -> image::RgbaImage {
    let mut br = paged_renderer::BytesResolver::new();
    br.add_image("picture.png", picture);
    let opts = PipelineOptions {
        assets: Some(&br),
        ..PipelineOptions::default()
    };
    let document = idml_import::import_idml_doc(idml).unwrap();
    let (_built, images) = pipeline::render_document(&document, &opts, dpi, Color::WHITE).unwrap();
    images.into_iter().next().unwrap()
}

fn is_green(p: image::Rgba<u8>) -> bool {
    p.0[0] < 80 && p.0[1] > 160 && p.0[2] < 80
}

fn is_white(p: image::Rgba<u8>) -> bool {
    p.0[0] > 240 && p.0[1] > 240 && p.0[2] > 240
}

/// A cover spelled as negative crops — the picture's box 20 pt wider
/// than the frame on each side — shows only inside the frame.
#[test]
fn a_negative_crop_overhang_is_clipped_to_the_frame() {
    let idml = package(
        r#"<Rectangle Self="r1" GeometricBounds="70 40 130 160" StrokeWeight="0">
      <FrameFittingOption LeftCrop="-20" TopCrop="0" RightCrop="-20" BottomCrop="0" FittingOnEmptyFrame="FillProportionally"/>
      <Image Self="i1" LinkResourceURI="picture.png"/>
    </Rectangle>"#,
    );
    let img = render(&idml, png(1, 1, |_, _| [0, 220, 0, 255]), 72.0);
    for (x, y) in [(42, 100), (100, 100), (158, 100)] {
        assert!(
            is_green(*img.get_pixel(x, y)),
            "({x},{y}) is inside the frame, got {:?}",
            img.get_pixel(x, y)
        );
    }
    for (x, y) in [(25, 100), (35, 100), (165, 100), (175, 100)] {
        assert!(
            is_white(*img.get_pixel(x, y)),
            "({x},{y}) is overhang outside the frame, got {:?}",
            img.get_pixel(x, y)
        );
    }
}

/// A frame whose edges fall between pixels draws its picture exactly as
/// the same frame does with the placement spelled as the transform an
/// IDML export writes for it — pixel for pixel, edges included.
#[test]
fn a_fractional_frame_draws_what_its_spelled_transform_draws() {
    let rect = |image_attrs: &str| {
        format!(
            r#"<Rectangle Self="r1" GeometricBounds="70.4 40.3 130.6 160.7" StrokeWeight="0">
      <FrameFittingOption LeftCrop="0" TopCrop="0.35" RightCrop="0" BottomCrop="0" FittingOnEmptyFrame="FitContentToFrame"/>
      <Image Self="i1" LinkResourceURI="picture.png"{image_attrs}/>
    </Rectangle>"#
        )
    };
    let (w, h) = (5u32, 3u32);
    let picture = || {
        png(w, h, |x, y| {
            [(40 * x + 20) as u8, (70 * y + 30) as u8, 200, 255]
        })
    };
    let bare = package(&rect(""));
    // The transform, from the bounds as the importer reads them, in the
    // writer's arithmetic: the box minus the crops, scaled from pixels.
    let b = idml_import::import_idml_doc(&bare).unwrap().spreads[0]
        .spread
        .rectangles[0]
        .bounds;
    let t = [
        (b.right - b.left - 0.0 - 0.0).max(0.0) / w as f32,
        0.0,
        0.0,
        (b.bottom - b.top - 0.35 - 0.0).max(0.0) / h as f32,
        b.left + 0.0,
        b.top + 0.35,
    ];
    let spelled = package(&rect(&format!(
        r#" ItemTransform="{} 0 0 {} {} {}""#,
        t[0], t[3], t[4], t[5]
    )));
    for dpi in [72.0, 150.0] {
        let a = render(&bare, picture(), dpi);
        let z = render(&spelled, picture(), dpi);
        let differing = a.pixels().zip(z.pixels()).filter(|(p, q)| p != q).count();
        assert_eq!(differing, 0, "{differing} pixels differ at {dpi} dpi");
    }
}
