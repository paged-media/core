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

//! W2.5 — element-level `Visible="false"` must hide a page item from
//! the render (the same skip a hidden *layer* gets). We stamp a cyan
//! foreground rectangle over a magenta background: with `Visible="true"`
//! the overlap reads cyan; with `Visible="false"` the foreground is
//! skipped entirely and the overlap reads the magenta background
//! through. `Locked` is NOT a render gate (locked items still paint) —
//! that selection-gating lives in the canvas hit-tester, exercised by
//! the canvas tests, not here.

use std::io::Write;

use paged_compose::Color;
use paged_renderer::{pipeline, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// Render a magenta background with a cyan foreground rectangle whose
/// `Visible` attribute is `fg_visible`. Returns the centre pixel of the
/// foreground rect (in the overlap region).
fn render_fg(fg_visible: bool) -> [u8; 4] {
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
  <Graphic>
    <Color Self="Color/Magenta" Name="Magenta" Model="Process"
           Space="CMYK" ColorValue="0 100 0 0"/>
    <Color Self="Color/Cyan" Name="Cyan" Model="Process"
           Space="CMYK" ColorValue="100 0 0 0"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    let visible_attr = if fg_visible {
        r#" Visible="true""#
    } else {
        r#" Visible="false""#
    };
    let spread_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 100 100"/>
    <Rectangle Self="rBg" GeometricBounds="0 0 100 100"
               FillColor="Color/Magenta" StrokeWeight="0"/>
    <Rectangle Self="rFg" GeometricBounds="20 20 80 80"
               FillColor="Color/Cyan" StrokeWeight="0"{visible}/>
  </Spread>
</idPkg:Spread>"#,
        visible = visible_attr,
    );
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(spread_xml.as_bytes()).unwrap();
    let bytes = zip.finish().unwrap().into_inner();

    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (_built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();
    // Page 100×100 at 72 dpi → 100 px; fg rect (20,20)..(80,80); centre (50,50).
    images[0].get_pixel(50, 50).0
}

#[test]
fn element_visible_false_hides_item_from_render() {
    let shown = render_fg(true);
    let hidden = render_fg(false);

    // Visible="true": the cyan foreground covers the overlap. Cyan in
    // sRGB ≈ (0, 255, 255) — green + blue high, red low.
    assert!(
        shown[1] > 200 && shown[2] > 200 && shown[0] < 80,
        "Visible=true overlap should read cyan; got {shown:?}"
    );

    // Visible="false": the foreground is skipped, so the magenta
    // background shows through. Magenta in sRGB ≈ (255, 0, 255) — red +
    // blue high, green LOW. The discriminating channel is green: cyan
    // keeps it high, magenta collapses it.
    assert!(
        hidden[0] > 200 && hidden[1] < 80,
        "Visible=false overlap should read magenta background through; got {hidden:?}"
    );
    assert!(
        hidden[1] < shown[1],
        "hiding the foreground must drop the green channel (cyan→magenta): \
         hidden green {} vs shown green {}",
        hidden[1],
        shown[1],
    );
}
