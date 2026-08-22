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

//! A rectangle that draws nothing must leave nothing behind — not one
//! command, and not one entry in the page's interned path pool.
//!
//! `emit_rectangle_into` used to resolve the path its frame-effects
//! stamp would use before it knew whether the rect declared any
//! effects. That resolution is not free of consequence: for a flat rect
//! it INTERNS the unit-rect path into `BuiltPage::list.paths`, and
//! `DisplayList::digest` — the "same code, same scene" tripwire that
//! `paged-canvas`'s render-effect sweep and `paged-sdk`'s
//! native-equivalence test both ride — folds the whole pool, not just
//! the commands. So a rect with no fill, no stroke and no effects (an
//! empty anchored frame, a bare placement box) moved the digest of a
//! page it had put no mark on.
//!
//! The sweep read that as `insertAnchoredFrame` "renders in the export
//! build but not on the canvas" and filed it as an invalidation defect.
//! It was neither: the live build simply had the unit rect in its pool
//! already, because the A5 substituted-font highlight is a rect. A
//! phantom pool entry is a lie the digest cannot see through, so the
//! intern is now conditional on the rect actually declaring effects.

use std::io::Write;

use paged_renderer::pipeline::{self, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One 595×842 page carrying `rects` verbatim as spread XML.
fn idml_with(rects: &str) -> Vec<u8> {
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
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" Self="d1">
  <idPkg:Graphic src="Resources/Graphic.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Ink" Model="Process" Space="RGB" ColorValue="10 20 30" Name="Ink"/>
</idPkg:Graphic>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" PageCount="1" ItemTransform="1 0 0 1 0 0">
    <Page Self="p1" GeometricBounds="0 0 841.89 595.276" ItemTransform="1 0 0 1 0 0"/>
    {rects}
  </Spread>
</idPkg:Spread>"#
        )
        .as_bytes(),
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

/// A 100×100 rectangle. `fill` / `stroke` go straight onto the element,
/// so `Swatch/None` + `StrokeWeight="0"` is the invisible shape the
/// anchored-frame door mints.
fn rect_xml(self_id: &str, fill: &str, stroke: &str, weight: &str) -> String {
    format!(
        r#"<Rectangle Self="{self_id}" Visible="true" ItemTransform="1 0 0 1 200 300"
                FillColor="{fill}" StrokeColor="{stroke}" StrokeWeight="{weight}">
      <Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
        <PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/>
        <PathPointType Anchor="0 100" LeftDirection="0 100" RightDirection="0 100"/>
        <PathPointType Anchor="100 100" LeftDirection="100 100" RightDirection="100 100"/>
        <PathPointType Anchor="100 0" LeftDirection="100 0" RightDirection="100 0"/>
      </PathPointArray></GeometryPathType></PathGeometry></Properties>
    </Rectangle>"#
    )
}

fn first_page(rects: &str) -> paged_renderer::BuiltPage {
    let doc = idml_import::import_idml_doc(&idml_with(rects)).expect("open");
    let built = pipeline::build_document(&doc, &PipelineOptions::default()).expect("build");
    built.pages.into_iter().next().expect("one page")
}

#[test]
fn a_rect_with_no_fill_no_stroke_and_no_effects_leaves_the_page_untouched() {
    let bare = first_page("");
    let invisible = first_page(&rect_xml("r_invisible", "Swatch/None", "Swatch/None", "0"));

    assert_eq!(
        invisible.list.commands.len(),
        bare.list.commands.len(),
        "an invisible rect emits no draw command"
    );
    assert_eq!(
        invisible.list.paths.len(),
        bare.list.paths.len(),
        "and interns no path either — a pool entry no command references \
         is invisible on the page and LOUD in `DisplayList::digest`, which \
         is how a mutation that paints nothing came to look like one that \
         paints"
    );
    assert_eq!(
        invisible.list.digest(),
        bare.list.digest(),
        "so the page's render identity is unchanged by adding it"
    );
}

#[test]
fn a_filled_rect_still_interns_its_path_and_paints() {
    let bare = first_page("");
    let filled = first_page(&rect_xml("r_filled", "Color/Ink", "Swatch/None", "0"));

    assert_eq!(
        filled.list.commands.len(),
        bare.list.commands.len() + 1,
        "the control: a rect that paints still emits its fill"
    );
    assert_eq!(
        filled.list.paths.len(),
        bare.list.paths.len() + 1,
        "and still interns the unit rect — the intern moved to the fill's \
         own call, it did not go away"
    );
}
