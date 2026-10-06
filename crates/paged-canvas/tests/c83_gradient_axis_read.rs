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

//! C-83b — a Polygon's and an Oval's gradient axis (fill and stroke angle
//! and length) were writable and exported but NOT READABLE: the panel and
//! paged.draw's Gradient Annotator could set an axis they could never
//! show (found by plugin-draw's InDesign round trip, which read the axis
//! InDesign got while the engine answered no row). Read and write are
//! pinned as a pair.

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

#[test]
fn a_polygon_and_an_oval_read_back_the_gradient_axis_they_were_given() {
    let paths = [
        (PropertyPath::FrameGradientFillAngle, 30.0),
        (PropertyPath::FrameGradientFillLength, 250.0),
        (PropertyPath::FrameGradientStrokeAngle, 45.0),
        (PropertyPath::FrameGradientStrokeLength, 120.5),
    ];
    for id in [
        ElementId::Polygon("pen".into()),
        ElementId::Oval("ov".into()),
    ] {
        let mut m = model();
        for (path, _) in &paths {
            assert_eq!(
                read(&m, &id, *path),
                Some(Value::Length(None)),
                "{id:?} {path:?} unset"
            );
        }
        for (path, v) in &paths {
            m.apply_mutation(&Mutation::SetElementProperty {
                element_id: id.clone(),
                path: *path,
                value: Value::Length(Some(*v)),
            })
            .expect("write");
            assert_eq!(
                read(&m, &id, *path),
                Some(Value::Length(Some(*v))),
                "{id:?} {path:?}"
            );
        }
        for _ in &paths {
            assert!(m.undo().is_some(), "undo");
        }
        for (path, _) in &paths {
            assert_eq!(
                read(&m, &id, *path),
                Some(Value::Length(None)),
                "{id:?} {path:?} after undo"
            );
        }
    }
}
