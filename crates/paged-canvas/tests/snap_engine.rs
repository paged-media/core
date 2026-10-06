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

//! v67 (RFI C-68) — snapping in the engine: the point resolver behind
//! `RequestSnapPoint`, and the gestures that use the same targets.
//!
//! The page (612 × 792) holds a square polygon at (100,100)–(200,200), an
//! oval spanning x 380–580 / y 400–500 and a rectangle at x 50–150 /
//! y 500–550, plus a vertical ruler guide at x = 300.

use std::io::Write;

use paged_canvas::channel::Mutation;
use paged_canvas::gesture::ResizeHandle;
use paged_canvas::snap_point::{SnapExclude, SnapPointQuery, SnapSettings, SnapSource};
use paged_canvas::{CanvasModel, CanvasOptions, ElementId, GestureModifiers, GestureType, PageId};
use paged_mutate::operation::GuideOrientationSpec;
use paged_mutate::{PathPointAddress, PathPointRole};

fn idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Polygon Self="poly1" GeometricBounds="0 0 100 100" ItemTransform="1 0 0 1 100 100">
  <Properties>
    <PathGeometry>
      <GeometryPathType pathOpen="false">
        <PathPointArray>
          <PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/>
          <PathPointType Anchor="100 0" LeftDirection="100 0" RightDirection="100 0"/>
          <PathPointType Anchor="100 100" LeftDirection="100 100" RightDirection="100 100"/>
          <PathPointType Anchor="0 100" LeftDirection="0 100" RightDirection="0 100"/>
        </PathPointArray>
      </GeometryPathType>
    </PathGeometry>
  </Properties>
</Polygon>
<Oval Self="o1" GeometricBounds="0 0 100 200" ItemTransform="1 0 0 1 380 400"/>
<Rectangle Self="r1" GeometricBounds="500 50 550 150" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    let mut m = CanvasModel::load("snap", &idml(), CanvasOptions::default()).expect("load");
    m.apply_mutation(&Mutation::InsertGuide {
        spread_id: "s1".into(),
        orientation: GuideOrientationSpec::Vertical,
        position: 300.0,
        page_index: 0,
    })
    .expect("guide");
    m
}

fn query(x: f32, y: f32) -> SnapPointQuery {
    SnapPointQuery {
        page_id: PageId("p1".into()),
        point: [x, y],
        camera_scale: Some(1.0),
        exclude: vec![],
        extra_points: vec![],
    }
}

fn rect_bounds(m: &CanvasModel) -> paged_model::Bounds {
    m.scene().spreads[0]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some("r1"))
        .expect("r1")
        .bounds
}

#[test]
fn a_query_lands_on_an_ovals_quadrant_point() {
    let mut m = model();
    let r = m.snap_point(&query(481.0, 402.0));
    assert_eq!(r.point, [480.0, 400.0]);
    let hit = r.point_target.expect("a point");
    assert_eq!(hit.source, SnapSource::AnchorPoint);
    assert_eq!(hit.element, Some(ElementId::Oval("o1".into())));
}

#[test]
fn a_query_reads_the_ruler_guide_and_another_paths_anchor_line() {
    let mut m = model();
    // x near the guide (300), y near the polygon's top anchors (100).
    let r = m.snap_point(&query(302.0, 103.0));
    assert_eq!(r.point, [300.0, 100.0]);
    assert_eq!(r.x_target.unwrap().source, SnapSource::Guide);
    let y = r.y_target.unwrap();
    assert_eq!(y.element, Some(ElementId::Polygon("poly1".into())));
}

#[test]
fn an_excluded_anchor_is_not_a_target_but_its_siblings_are() {
    let mut m = model();
    let mut q = query(201.0, 199.0);
    q.exclude = vec![SnapExclude {
        id: ElementId::Polygon("poly1".into()),
        anchors: Some(vec![2]),
    }];
    let r = m.snap_point(&q);
    // Not the excluded corner (200,200) as a point; the lines through the
    // other corners (x = 200 from anchor 1, y = 200 from anchor 3) still
    // pull it there.
    assert!(r.point_target.is_none());
    assert_eq!(r.point, [200.0, 200.0]);
}

#[test]
fn translate_now_aligns_with_an_oval() {
    let mut m = model();
    let h = m
        .begin_gesture_with_scale(
            vec![ElementId::Rectangle("r1".into())],
            GestureType::Translate,
            None,
            Some(1.0),
        )
        .expect("begin");
    // Left edge 50 + 328 = 378: two points short of the oval's left (380).
    let u = m
        .update_gesture(h, (328.0, 0.0), GestureModifiers::default())
        .unwrap();
    assert!(!u.snap_lines.is_empty());
    m.commit_gesture(h).unwrap();
    assert!((rect_bounds(&m).left - 380.0).abs() < 1e-3);
}

#[test]
fn resize_lands_the_moving_edge_on_a_guide() {
    let mut m = model();
    let h = m
        .begin_gesture_with_scale(
            vec![ElementId::Rectangle("r1".into())],
            GestureType::Resize {
                handle: ResizeHandle::East,
            },
            None,
            Some(1.0),
        )
        .expect("begin");
    // Right edge 150 + 147 = 297: three short of the guide.
    m.update_gesture(h, (147.0, 0.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    let b = rect_bounds(&m);
    assert!((b.right - 300.0).abs() < 1e-3, "right = {}", b.right);
    assert!((b.left - 50.0).abs() < 1e-3);
}

#[test]
fn a_path_edit_drag_is_pulled_onto_another_objects_lines() {
    let mut m = model();
    let address = PathPointAddress {
        index: 2,
        role: PathPointRole::Anchor,
    };
    let h = m
        .begin_gesture_with_scale(
            vec![ElementId::Polygon("poly1".into())],
            GestureType::PathEdit { address },
            None,
            Some(1.0),
        )
        .expect("begin");
    // (200,200) + (178,201) = (378,401): x 2 from the oval's left
    // quadrant line (380), y 1 from its top quadrant line (400) — and 5
    // from the page's centre line (396), which is the nearer line at 397.
    m.update_gesture(h, (178.0, 201.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    let a = m.scene().spreads[0]
        .spread
        .polygons
        .iter()
        .find(|p| p.self_id.as_deref() == Some("poly1"))
        .unwrap()
        .anchors[2];
    // Local = page − (100,100).
    assert!((a.anchor.0 - 280.0).abs() < 1e-3, "x = {}", a.anchor.0);
    assert!((a.anchor.1 - 300.0).abs() < 1e-3, "y = {}", a.anchor.1);
}

#[test]
fn snapping_off_leaves_every_gesture_where_the_pointer_put_it() {
    let mut m = model();
    m.set_snap_settings(SnapSettings {
        enabled: false,
        ..SnapSettings::default()
    });
    assert!(!m.snap_point(&query(481.0, 402.0)).snapped);
    let h = m
        .begin_gesture_with_scale(
            vec![ElementId::Rectangle("r1".into())],
            GestureType::Translate,
            None,
            Some(1.0),
        )
        .expect("begin");
    m.update_gesture(h, (328.0, 0.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    assert!((rect_bounds(&m).left - 378.0).abs() < 1e-3);
}

#[test]
fn the_index_follows_an_edit() {
    let mut m = model();
    assert!(m.snap_point(&query(481.0, 402.0)).snapped);
    // Move the oval away; its old quadrant point is no longer a target.
    m.apply_mutation(&Mutation::DeleteFrame {
        frame_id: "o1".into(),
    })
    .expect("delete");
    let r = m.snap_point(&query(481.0, 402.0));
    assert!(r.point_target.is_none());
}
