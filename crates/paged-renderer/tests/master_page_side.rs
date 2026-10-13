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

//! A body page takes the master page on its own side of the spine.
//!
//! The master pass picked the master page by the body page's index in its
//! spread (`local index → master page index`). That is right for a facing
//! spread laid out like its master, and wrong for a page standing alone:
//! a single-page spread with `BindingLocation="0"` holds a RIGHT-hand page
//! (it sits right of the spine, at x ≥ 0), yet as index 0 it took the
//! master's LEFT page. InDesign draws the right one — page 61 of the
//! annual, cut to one page, lost its running head (`MMXXVI · A SPECIMEN
//! ANNUAL`, on the master's right page) only in Paged's render.
//!
//! The master below paints red on its left page and blue on its right
//! page, at the same page position; each body page must show its side's
//! colour there.

use std::io::Write;

use paged_compose::Color;
use paged_renderer::pipeline::{self, PipelineOptions};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const W: f32 = 200.0;

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

/// A page whose left edge sits at spread x = `x`.
fn page(id: &str, x: f32) -> String {
    format!(
        r#"<Page Self="{id}" AppliedMaster="m1" GeometricBounds="0 0 {W} {W}" ItemTransform="1 0 0 1 {x} 0" MasterPageTransform="1 0 0 1 0 0"/>"#
    )
}

fn spread(id: &str, binding: u32, pages: &[(&str, f32)]) -> String {
    let pages: String = pages.iter().map(|(p, x)| page(p, *x)).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="{id}" PageCount="{n}" BindingLocation="{binding}" ItemTransform="1 0 0 1 0 0">{pages}</Spread>
</idPkg:Spread>"#,
        n = pages.matches("<Page ").count()
    )
}

/// Spreads, in document order:
/// 1. one page right of the spine (`BindingLocation="0"`) — a recto alone;
/// 2. a facing pair, verso then recto (`BindingLocation="1"`);
/// 3. one page left of the spine (`BindingLocation="1"`) — a verso alone.
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
  <idPkg:Spread src="Spreads/Spread_sa.xml"/>
  <idPkg:Spread src="Spreads/Spread_sb.xml"/>
  <idPkg:Spread src="Spreads/Spread_sc.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Red" Model="Process" Space="RGB" ColorValue="230 0 0" Name="Red"/>
  <Color Self="Color/Blue" Model="Process" Space="RGB" ColorValue="0 0 230" Name="Blue"/>
</idPkg:Graphic>"#,
    )
    .unwrap();
    // A facing master, spine at x = 0: left page at -W, right page at 0.
    let master = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:MasterSpread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <MasterSpread Self="m1" Name="C-Opener" PageCount="2" ItemTransform="1 0 0 1 0 0">
    <Page Self="m1l" GeometricBounds="0 0 {W} {W}" ItemTransform="1 0 0 1 -{W} 0"/>
    <Page Self="m1r" GeometricBounds="0 0 {W} {W}" ItemTransform="1 0 0 1 0 0"/>
    <Polygon Self="left" FillColor="Color/Red" StrokeWeight="0" ItemTransform="1 0 0 1 0 0">{left}</Polygon>
    <Polygon Self="right" FillColor="Color/Blue" StrokeWeight="0" ItemTransform="1 0 0 1 0 0">{right}</Polygon>
  </MasterSpread>
</idPkg:MasterSpread>"#,
        left = square(-150.0, 50.0, -50.0, 150.0),
        right = square(50.0, 50.0, 150.0, 150.0),
    );
    zip.start_file("MasterSpreads/MasterSpread_m1.xml", deflated)
        .unwrap();
    zip.write_all(master.as_bytes()).unwrap();
    for (name, body) in [
        ("Spreads/Spread_sa.xml", spread("sa", 0, &[("recto", 0.0)])),
        (
            "Spreads/Spread_sb.xml",
            spread("sb", 1, &[("verso2", -W), ("recto2", 0.0)]),
        ),
        ("Spreads/Spread_sc.xml", spread("sc", 1, &[("verso", -W)])),
    ] {
        zip.start_file(name, deflated).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn colour_at_centre(img: &image::RgbaImage) -> &'static str {
    let [r, g, b, _] = img.get_pixel(100, 100).0;
    match (r > 180 && g < 60 && b < 60, b > 180 && r < 60 && g < 60) {
        (true, _) => "red (master left page)",
        (_, true) => "blue (master right page)",
        _ => "neither",
    }
}

#[test]
fn every_body_page_takes_the_master_page_on_its_own_side_of_the_spine() {
    let document = idml_import::import_idml_doc(&build()).unwrap();
    let (_, images) =
        pipeline::render_document(&document, &PipelineOptions::default(), 72.0, Color::WHITE)
            .unwrap();
    assert_eq!(images.len(), 4);
    let got: Vec<&str> = images.iter().map(colour_at_centre).collect();
    assert_eq!(
        got,
        [
            "blue (master right page)", // a recto standing alone
            "red (master left page)",   // the facing pair's verso…
            "blue (master right page)", // …and recto
            "red (master left page)",   // a verso standing alone
        ]
    );
}
