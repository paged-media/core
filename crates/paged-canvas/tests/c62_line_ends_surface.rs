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

//! C-62 — a pen path (an open `<Polygon>`) reads, writes and undoes its
//! stroke end cap and line ends through the EXISTING property paths, and
//! the cap reaches `<GraphicLine>` and `<Oval>` too. Read and write are
//! pinned as a pair (the C-17 lesson): every row read here is written.

use std::io::Write;

use paged_canvas::{channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions};
use paged_mutate::{PropertyPath, Value};

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
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Polygon Self="pen" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="4" EndCap="RoundEndCap" LeftLineEnd="CircleSolidArrowHead" RightLineEnd="None"><Properties><PathGeometry><GeometryPathType PathOpen="true"><PathPointArray><PathPointType Anchor="100 100" LeftDirection="100 100" RightDirection="100 100"/><PathPointType Anchor="200 160" LeftDirection="200 160" RightDirection="200 160"/></PathPointArray></GeometryPathType></PathGeometry></Properties></Polygon>
<GraphicLine Self="gl" GeometricBounds="300 100 300 300" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="2" EndCap="ProjectingEndCap"/>
<Oval Self="ov" GeometricBounds="400 100 460 160" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="2" EndCap="ButtEndCap"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("d1", &idml(), CanvasOptions::default()).expect("load")
}

fn read(m: &CanvasModel, id: &ElementId, path: PropertyPath) -> Option<Value> {
    m.element_properties(id)
        .expect("element_properties")
        .entries
        .iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("{id:?} has no {path:?} row"))
        .value
        .clone()
}

fn text(s: &str) -> Option<Value> {
    Some(Value::Text(s.to_string()))
}

#[test]
fn every_path_kind_reads_its_cap_and_a_pen_path_its_line_ends() {
    let m = model();
    let pen = ElementId::Polygon("pen".into());
    assert_eq!(
        read(&m, &pen, PropertyPath::FrameStrokeEndCap),
        text("RoundEndCap")
    );
    assert_eq!(
        read(&m, &pen, PropertyPath::FrameStrokeStartArrowhead),
        text("CircleSolidArrowHead")
    );
    // InDesign's explicit "None" reads as the cleared spelling.
    assert_eq!(
        read(&m, &pen, PropertyPath::FrameStrokeEndArrowhead),
        text("")
    );
    assert_eq!(
        read(
            &m,
            &ElementId::GraphicLine("gl".into()),
            PropertyPath::FrameStrokeEndCap
        ),
        text("ProjectingEndCap")
    );
    assert_eq!(
        read(
            &m,
            &ElementId::Oval("ov".into()),
            PropertyPath::FrameStrokeEndCap
        ),
        text("ButtEndCap")
    );
}

#[test]
fn each_row_writes_reads_back_and_undoes() {
    let cases = [
        (
            ElementId::Polygon("pen".into()),
            PropertyPath::FrameStrokeEndCap,
            "ProjectingEndCap",
        ),
        (
            ElementId::Polygon("pen".into()),
            PropertyPath::FrameStrokeStartArrowhead,
            "BarbedArrowHead",
        ),
        (
            ElementId::Polygon("pen".into()),
            PropertyPath::FrameStrokeEndArrowhead,
            "TriangleArrowHead",
        ),
        (
            ElementId::GraphicLine("gl".into()),
            PropertyPath::FrameStrokeEndCap,
            "RoundEndCap",
        ),
        (
            ElementId::Oval("ov".into()),
            PropertyPath::FrameStrokeEndCap,
            "RoundEndCap",
        ),
    ];
    for (id, path, token) in cases {
        let mut m = model();
        let before = read(&m, &id, path);
        m.apply_mutation(&Mutation::SetElementProperty {
            element_id: id.clone(),
            path,
            value: Value::Text(token.to_string()),
        })
        .unwrap_or_else(|e| panic!("{id:?} {path:?}: {e:?}"));
        assert_eq!(
            read(&m, &id, path),
            text(token),
            "{id:?} {path:?} read-back"
        );
        assert!(m.undo().is_some(), "{id:?} {path:?} undo");
        assert_eq!(read(&m, &id, path), before, "{id:?} {path:?} undo restores");
    }
}
