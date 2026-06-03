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

//! Editor-ops — Shear gesture integration tests (the Shear tool).
//! Mirrors `rotate_scale_gesture.rs`: begin → update (SAB-style
//! deltas) → commit, asserting the composed `FrameTransform`, the
//! pivot fixed-point, the Shift 15°-snap, cancel, and undo/redo.

use std::io::Write;

use paged_canvas::{
    CanvasModel, CanvasOptions, ElementId, GestureAnchor, GestureError, GestureModifiers,
    GestureType,
};

fn small_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package").unwrap();
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
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 300 300" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>x</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load")
}

fn tf_transform(m: &CanvasModel, id: &str) -> Option<[f32; 6]> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some(id))
        .and_then(|f| f.item_transform)
}

fn anchor_at(point: (f32, f32)) -> GestureAnchor {
    GestureAnchor {
        page_id: paged_renderer::PageId("p1".into()),
        point_in_page: point,
    }
}

#[test]
fn shear_about_centroid_yields_expected_matrix() {
    // tf1 bounds [100, 100, 300, 300] → centroid pivot (200, 200).
    // Grab the TOP edge midpoint (200, 100): lever = 100 − 200 = −100.
    // Drag +50 in x → k = 50 / −100 = −0.5.
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Shear,
            Some(anchor_at((200.0, 100.0))),
        )
        .expect("begin");
    m.update_gesture(h, (50.0, 0.0), GestureModifiers::default())
        .expect("update");
    m.commit_gesture(h).expect("commit");

    let mt = tf_transform(&m, "tf1").expect("transform present");
    // Identity composed with x-shear k: [1, 0, k, 1, −k·pivot.y, 0].
    let k = -0.5_f32;
    assert!((mt[0] - 1.0).abs() < 1e-3, "a={}", mt[0]);
    assert!(mt[1].abs() < 1e-3, "b={}", mt[1]);
    assert!((mt[2] - k).abs() < 1e-3, "c={}", mt[2]);
    assert!((mt[3] - 1.0).abs() < 1e-3, "d={}", mt[3]);
    // The pivot (200, 200) is a fixed point.
    let (cx, cy) = (200.0_f32, 200.0_f32);
    let mapped_x = mt[0] * cx + mt[2] * cy + mt[4];
    let mapped_y = mt[1] * cx + mt[3] * cy + mt[5];
    assert!((mapped_x - cx).abs() < 1e-2, "cx mapped to {mapped_x}");
    assert!((mapped_y - cy).abs() < 1e-2, "cy mapped to {mapped_y}");
    // The grabbed point (200, 100) followed the pointer in x:
    // x' = 200 + k·(100 − 200) = 250.
    let gx = mt[0] * 200.0 + mt[2] * 100.0 + mt[4];
    assert!((gx - 250.0).abs() < 1e-2, "grabbed x mapped to {gx}");
}

#[test]
fn shear_shift_snaps_to_15_degree_angles() {
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Shear,
            Some(anchor_at((200.0, 100.0))),
        )
        .expect("begin");
    // Raw k = −36.4 / −100 = 0.364 → atan ≈ 20°; Shift snaps to 15°.
    m.update_gesture(
        h,
        (-36.4, 0.0),
        GestureModifiers {
            shift: true,
            ..Default::default()
        },
    )
    .expect("update");
    m.commit_gesture(h).expect("commit");
    let mt = tf_transform(&m, "tf1").expect("transform present");
    let expected = (std::f32::consts::PI / 12.0).tan(); // tan 15°
    assert!(
        (mt[2] - expected).abs() < 1e-3,
        "c={} expected tan15°={expected}",
        mt[2]
    );
}

#[test]
fn shear_cancel_restores_snapshot() {
    let mut m = model();
    let before = tf_transform(&m, "tf1");
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Shear,
            Some(anchor_at((200.0, 100.0))),
        )
        .expect("begin");
    m.update_gesture(h, (80.0, 0.0), GestureModifiers::default())
        .expect("update");
    m.cancel_gesture(h).expect("cancel");
    assert_eq!(
        tf_transform(&m, "tf1"),
        before,
        "cancel must restore the pre-gesture transform"
    );
}

#[test]
fn shear_undo_redo_round_trips() {
    let mut m = model();
    let before = tf_transform(&m, "tf1");
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Shear,
            Some(anchor_at((200.0, 100.0))),
        )
        .expect("begin");
    m.update_gesture(h, (50.0, 0.0), GestureModifiers::default())
        .expect("update");
    m.commit_gesture(h).expect("commit");
    let sheared = tf_transform(&m, "tf1").expect("sheared");

    m.undo().expect("undo");
    assert_eq!(tf_transform(&m, "tf1"), before, "undo restores the prior transform");
    m.redo().expect("redo");
    assert_eq!(
        tf_transform(&m, "tf1"),
        Some(sheared),
        "redo re-applies the shear"
    );
}

#[test]
fn shear_without_anchor_is_rejected() {
    let mut m = model();
    let err = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Shear,
            None,
        )
        .expect_err("shear needs an anchor");
    assert!(
        matches!(err, GestureError::MissingAnchor),
        "expected MissingAnchor, got {err:?}"
    );
}
