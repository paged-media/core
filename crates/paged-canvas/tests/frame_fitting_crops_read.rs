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

//! `frameFittingCrops` reads in the engine's convention on an
//! InDesign-authored document.
//!
//! The engine's crops are frame-space insets from the frame's edge to the
//! picture's (what `frameFittingCrops` writes and the renderer places
//! by). InDesign's `<FrameFittingOption>` crops are measured from the
//! picture's edge, in the picture's own units, positive where the frame
//! cuts in — a letterboxed frame carries NEGATIVE crops. The importer
//! read those raw, so the property reported a letterbox band of 30 pt as
//! -60, and setting the value it read back would have moved the picture.
//! It now converts by the picture's transform.

use std::io::Write;

use paged_canvas::{channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions};
use paged_mutate::{PropertyPath, Value};

/// A frame as InDesign writes one after Fit Content Proportionally: 160 ×
/// 100 pt, around a 200 × 200 picture at half size — 100 × 100 pt,
/// centred, a 30 pt band each side — with `LeftCrop` / `RightCrop` = the
/// band over the scale, negative.
const FRAME: &str = r#"<Rectangle Self="r1" ContentType="GraphicType" ItemTransform="1 0 0 1 100 200"><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray><PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/><PathPointType Anchor="0 100" LeftDirection="0 100" RightDirection="0 100"/><PathPointType Anchor="160 100" LeftDirection="160 100" RightDirection="160 100"/><PathPointType Anchor="160 0" LeftDirection="160 0" RightDirection="160 0"/></PathPointArray></GeometryPathType></PathGeometry></Properties><FrameFittingOption LeftCrop="-60" RightCrop="-60" FittingOnEmptyFrame="Proportionally"/><Image Self="i1" ItemTransform="0.5 0 0 0.5 30 0"><Properties><GraphicBounds Left="0" Top="0" Right="200" Bottom="200"/></Properties><Link Self="l1" LinkResourceURI="file:/Links/picture.png" StoredState="Normal"/></Image></Rectangle>"#;

fn idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
{FRAME}
</Spread></idPkg:Spread>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn crops(m: &CanvasModel) -> Option<Value> {
    m.element_properties(&ElementId::Rectangle("r1".into()))
        .expect("element_properties")
        .entries
        .iter()
        .find(|e| e.path == PropertyPath::FrameFittingCrops)
        .expect("a frameFittingCrops row")
        .value
        .clone()
}

/// `[top, left, bottom, right]`: the 30 pt bands, as frame-space insets.
#[test]
fn an_indesign_letterbox_reads_as_its_frame_space_band() {
    let m = CanvasModel::load("d1", &idml(), CanvasOptions::default()).expect("load");
    assert_eq!(crops(&m), Some(Value::Bounds([0.0, 30.0, 0.0, 30.0])));
}

/// The value the property reads is the value to write back: setting it
/// unchanged and exporting writes InDesign's own number again.
#[test]
fn writing_back_what_it_reads_exports_indesigns_crops() {
    let mut m = CanvasModel::load("d1", &idml(), CanvasOptions::default()).expect("load");
    let read = crops(&m).expect("crops");
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::Rectangle("r1".into()),
        path: PropertyPath::FrameFittingCrops,
        value: read.clone(),
    })
    .expect("write");
    let exported = m.export_idml().expect("export");
    let again = CanvasModel::load("d2", &exported, CanvasOptions::default()).expect("reload");
    assert_eq!(crops(&again), Some(read));
    let spread = {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&exported)).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("Spreads/Spread_s1.xml").unwrap(), &mut s)
            .unwrap();
        s
    };
    assert!(
        spread.contains(r#"LeftCrop="-60""#) && spread.contains(r#"RightCrop="-60""#),
        "{spread}"
    );
}
