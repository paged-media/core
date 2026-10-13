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

//! `ShowMasterItems` belongs to the spread. InDesign 20.0.1 writes it only
//! on `<Spread>` / `<MasterSpread>` and ignores a `<Page>`'s own value: a
//! spread that shows its master's items prints them whatever its page says,
//! and one that hides them hides them (tools/indesign-export/
//! master-items-probe, two packages rewritten so the spread and the page
//! disagree).

use std::io::Write;

use paged_compose::Color;
use paged_renderer::pipeline::{self, PipelineOptions};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// One page under a one-page master that paints a blue square at its
/// centre; `spread` and `page` are the two ShowMasterItems values.
fn build(spread: &str, page: &str) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut put = |name: &str, body: &str| {
        let opts = if name == "mimetype" { stored } else { deflated };
        zip.start_file(name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put("mimetype", "application/vnd.adobe.indesign-idml-package");
    put(
        "designmap.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Graphic src="Resources/Graphic.xml"/>
  <idPkg:MasterSpread src="MasterSpreads/MasterSpread_m1.xml"/>
  <idPkg:Spread src="Spreads/Spread_s1.xml"/>
</Document>"#,
    );
    put(
        "Resources/Graphic.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Blue" Model="Process" Space="RGB" ColorValue="0 0 230" Name="Blue"/>
</idPkg:Graphic>"#,
    );
    let pt = |x: f32, y: f32| {
        format!(
            r#"<PathPointType Anchor="{x} {y}" LeftDirection="{x} {y}" RightDirection="{x} {y}"/>"#
        )
    };
    put(
        "MasterSpreads/MasterSpread_m1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:MasterSpread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <MasterSpread Self="m1" Name="A-Master" PageCount="1" ShowMasterItems="true" ItemTransform="1 0 0 1 0 0">
    <Page Self="m1p" GeometricBounds="0 0 200 200" ItemTransform="1 0 0 1 0 0"/>
    <Polygon Self="square" FillColor="Color/Blue" StrokeWeight="0" ItemTransform="1 0 0 1 0 0"><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>{}{}{}{}</PathPointArray></GeometryPathType></PathGeometry></Properties></Polygon>
  </MasterSpread>
</idPkg:MasterSpread>"#,
            pt(50.0, 50.0),
            pt(150.0, 50.0),
            pt(150.0, 150.0),
            pt(50.0, 150.0)
        ),
    );
    put(
        "Spreads/Spread_s1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="s1" PageCount="1" BindingLocation="0" ShowMasterItems="{spread}" ItemTransform="1 0 0 1 0 0">
    <Page Self="p1" ShowMasterItems="{page}" AppliedMaster="m1" GeometricBounds="0 0 200 200" ItemTransform="1 0 0 1 0 0" MasterPageTransform="1 0 0 1 0 0"/>
  </Spread>
</idPkg:Spread>"#
        ),
    );
    zip.finish().unwrap().into_inner()
}

fn prints_the_master(spread: &str, page: &str) -> bool {
    let document = idml_import::import_idml_doc(&build(spread, page)).unwrap();
    let (_, images) =
        pipeline::render_document(&document, &PipelineOptions::default(), 72.0, Color::WHITE)
            .unwrap();
    let [r, g, b, _] = images[0].get_pixel(100, 100).0;
    b > 180 && r < 60 && g < 60
}

#[test]
fn the_spread_decides_whether_master_items_print() {
    assert!(prints_the_master("true", "true"));
    assert!(
        prints_the_master("true", "false"),
        "InDesign prints the master's items when the spread shows them, whatever the page says"
    );
    assert!(
        !prints_the_master("false", "true"),
        "InDesign hides the master's items when the spread hides them, whatever the page says"
    );
    assert!(!prints_the_master("false", "false"));
}
