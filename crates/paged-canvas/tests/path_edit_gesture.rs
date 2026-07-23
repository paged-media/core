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

//! Phase H — path-point editing integration tests.

use std::io::Write;

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, GestureModifiers, GestureType};
use paged_mutate::{PathPointAddress, PathPointRole};

fn small_idml() -> Vec<u8> {
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
        // A quad polygon, 4 anchors, all with collinear left/right
        // direction handles (so the path is straight-edged).
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Polygon Self="poly1" GeometricBounds="0 0 100 100" ItemTransform="1 0 0 1 0 0">
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
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load")
}

fn poly_anchor(m: &CanvasModel, id: &str, idx: usize) -> paged_model::PathAnchor {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .find(|p| p.self_id.as_deref() == Some(id))
        .and_then(|p| p.anchors.get(idx).copied())
        .expect("anchor")
}

#[test]
fn path_edit_anchor_translates_anchor_and_handles_together() {
    let mut m = model();
    let before = poly_anchor(&m, "poly1", 1);
    let address = PathPointAddress {
        index: 1,
        role: PathPointRole::Anchor,
    };
    let h = m
        .begin_gesture(
            vec![ElementId::Polygon("poly1".into())],
            GestureType::PathEdit { address },
            None,
        )
        .expect("begin");
    m.update_gesture(h, (10.0, -5.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    let after = poly_anchor(&m, "poly1", 1);
    // Anchor shifted by (10, -5).
    assert!((after.anchor.0 - before.anchor.0 - 10.0).abs() < 1e-3);
    assert!((after.anchor.1 - before.anchor.1 + 5.0).abs() < 1e-3);
    // Both handles shifted by the same delta (curve preserved).
    assert!((after.left.0 - before.left.0 - 10.0).abs() < 1e-3);
    assert!((after.right.1 - before.right.1 + 5.0).abs() < 1e-3);
}

#[test]
fn path_edit_left_handle_moves_only_that_handle() {
    let mut m = model();
    let before = poly_anchor(&m, "poly1", 2);
    let address = PathPointAddress {
        index: 2,
        role: PathPointRole::Left,
    };
    let h = m
        .begin_gesture(
            vec![ElementId::Polygon("poly1".into())],
            GestureType::PathEdit { address },
            None,
        )
        .unwrap();
    m.update_gesture(h, (-7.0, 9.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    let after = poly_anchor(&m, "poly1", 2);
    // Left handle moved; anchor + right unchanged.
    assert!((after.left.0 - before.left.0 + 7.0).abs() < 1e-3);
    assert!((after.left.1 - before.left.1 - 9.0).abs() < 1e-3);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.right, before.right);
}

#[test]
fn path_edit_undo_round_trips() {
    let mut m = model();
    let before = poly_anchor(&m, "poly1", 0);
    let address = PathPointAddress {
        index: 0,
        role: PathPointRole::Anchor,
    };
    let h = m
        .begin_gesture(
            vec![ElementId::Polygon("poly1".into())],
            GestureType::PathEdit { address },
            None,
        )
        .unwrap();
    m.update_gesture(h, (15.0, 15.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    m.undo().expect("undo");
    let restored = poly_anchor(&m, "poly1", 0);
    assert_eq!(restored.anchor, before.anchor);
    assert_eq!(restored.left, before.left);
    assert_eq!(restored.right, before.right);
}

#[test]
fn path_edit_cancel_restores() {
    let mut m = model();
    let before = poly_anchor(&m, "poly1", 3);
    let address = PathPointAddress {
        index: 3,
        role: PathPointRole::Right,
    };
    let h = m
        .begin_gesture(
            vec![ElementId::Polygon("poly1".into())],
            GestureType::PathEdit { address },
            None,
        )
        .unwrap();
    m.update_gesture(h, (50.0, 50.0), GestureModifiers::default())
        .unwrap();
    m.cancel_gesture(h).expect("cancel");
    let restored = poly_anchor(&m, "poly1", 3);
    assert_eq!(restored.right, before.right);
}
