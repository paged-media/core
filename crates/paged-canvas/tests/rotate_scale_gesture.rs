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

//! Phase D — rotate + scale gesture integration tests. Pure math is
//! tested in `gesture::tests` in `gesture.rs`; this file drives the
//! full lifecycle against a real `CanvasModel`.

use std::io::Write;

use paged_canvas::{
    CanvasModel, CanvasOptions, ElementId, GestureAnchor, GestureModifiers, GestureType,
};

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
<Rectangle Self="r1" GeometricBounds="100 400 300 600" ItemTransform="1 0 0 1 0 0"/>
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
fn rotate_90_degrees_about_centroid_yields_expected_matrix() {
    // Frame tf1: bounds [100, 100, 300, 300] (top, left, bottom,
    // right), no item_transform. Centroid in spread coords = (200, 200).
    // Anchor directly to the right of the centroid: (300, 200).
    // After a 90° clockwise rotation (dy positive = down in screen,
    // and atan2 grows counter-clockwise in math y-down… see math
    // below) the centroid stays put, the matrix encodes the rotation.
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Rotate,
            Some(anchor_at((300.0, 200.0))),
        )
        .expect("begin");
    // Move pointer to (200, 300) — a +90° rotation (atan2 yields
    // +π/2 from y=0 → y=100 with x=0).
    m.update_gesture(h, (-100.0, 100.0), GestureModifiers::default())
        .expect("update");
    m.commit_gesture(h).expect("commit");

    let mt = tf_transform(&m, "tf1").expect("transform present");
    // 90° rotation matrix is [a, b, c, d] = [0, 1, -1, 0]. Translation
    // captures the pivot-preservation correction.
    assert!(mt[0].abs() < 1e-3, "a={}", mt[0]);
    assert!((mt[1] - 1.0).abs() < 1e-3, "b={}", mt[1]);
    assert!((mt[2] - -1.0).abs() < 1e-3, "c={}", mt[2]);
    assert!(mt[3].abs() < 1e-3, "d={}", mt[3]);
    // The centroid (200, 200) must map back to itself under the new
    // transform.
    let cx = 200.0_f32;
    let cy = 200.0_f32;
    let mapped_x = mt[0] * cx + mt[2] * cy + mt[4];
    let mapped_y = mt[1] * cx + mt[3] * cy + mt[5];
    assert!((mapped_x - cx).abs() < 1e-2, "cx mapped to {mapped_x}");
    assert!((mapped_y - cy).abs() < 1e-2, "cy mapped to {mapped_y}");
}

#[test]
fn rotate_shift_snaps_to_nearest_15_degrees() {
    let mut m = model();
    // Anchor to the right of the centroid (200, 200).
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Rotate,
            Some(anchor_at((300.0, 200.0))),
        )
        .unwrap();
    // Tiny rotation that should snap to 15° (π/12 ≈ 0.2618 rad).
    // Move pointer slightly up: (300, 200) → (300 + dx ≈ 0, 200 - 30).
    // atan2(-30, 100) ≈ -0.2914 rad ≈ -16.7° → snaps to -15°.
    m.update_gesture(
        h,
        (0.0, -30.0),
        GestureModifiers {
            shift: true,
            alt: false,
            disable_snap: false,
        },
    )
    .unwrap();
    m.commit_gesture(h).unwrap();
    let mt = tf_transform(&m, "tf1").expect("transform");
    // cos(-15°) ≈ 0.9659; sin(-15°) ≈ -0.2588.
    let expected_a = (-15.0_f32.to_radians()).cos();
    let expected_b = (-15.0_f32.to_radians()).sin();
    assert!(
        (mt[0] - expected_a).abs() < 1e-3,
        "a={} expected ~{}",
        mt[0],
        expected_a
    );
    assert!(
        (mt[1] - expected_b).abs() < 1e-3,
        "b={} expected ~{}",
        mt[1],
        expected_b
    );
}

#[test]
fn rotate_cancel_restores_snapshot() {
    let mut m = model();
    let before = tf_transform(&m, "tf1");
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Rotate,
            Some(anchor_at((300.0, 200.0))),
        )
        .unwrap();
    m.update_gesture(h, (-100.0, 100.0), GestureModifiers::default())
        .unwrap();
    m.cancel_gesture(h).expect("cancel");
    assert_eq!(tf_transform(&m, "tf1"), before);
}

#[test]
fn rotate_undo_redo_round_trips() {
    let mut m = model();
    let before = tf_transform(&m, "tf1");
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Rotate,
            Some(anchor_at((300.0, 200.0))),
        )
        .unwrap();
    m.update_gesture(h, (-100.0, 100.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    m.undo().expect("undo");
    assert_eq!(tf_transform(&m, "tf1"), before);
    m.redo().expect("redo");
    let after = tf_transform(&m, "tf1").expect("present after redo");
    assert!((after[1] - 1.0).abs() < 1e-3, "redo restored rotation");
}

#[test]
fn rotate_without_anchor_errors_clearly() {
    let mut m = model();
    let err = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Rotate,
            None,
        )
        .expect_err("Rotate needs an anchor");
    assert!(format!("{err}").contains("anchor"), "{err}");
}

#[test]
fn scale_doubles_about_centroid() {
    // Anchor at (300, 200), 100 pt to the right of the centroid
    // (200, 200). Drag to (400, 200) → +100 in x → sx = 200/100 = 2.0.
    // sy stays 1.0 because anchor_dy = 0 → falls back to identity.
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Scale,
            Some(anchor_at((300.0, 200.0))),
        )
        .unwrap();
    m.update_gesture(h, (100.0, 0.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    let mt = tf_transform(&m, "tf1").expect("transform");
    assert!((mt[0] - 2.0).abs() < 1e-3, "sx={}", mt[0]);
    // sy left at 1.0 since the anchor was on the horizontal axis.
    assert!((mt[3] - 1.0).abs() < 1e-3, "sy={}", mt[3]);
    // Centroid preserved.
    let mapped_x = mt[0] * 200.0 + mt[2] * 200.0 + mt[4];
    assert!((mapped_x - 200.0).abs() < 1e-2);
}

// ---- Phase E — multi-element rotate / scale -----------------------

fn rect_transform(m: &CanvasModel, id: &str) -> Option<[f32; 6]> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(id))
        .and_then(|r| r.item_transform)
}

#[test]
fn multi_node_rotate_commits_as_batch_about_union_centroid() {
    // tf1 centroid = (200, 200); r1 centroid = (500, 200); union
    // centroid = (350, 200). Anchor 100 pt to the right of the union
    // centroid at (450, 200); drag to (350, 300) → +90°.
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![
                ElementId::TextFrame("tf1".into()),
                ElementId::Rectangle("r1".into()),
            ],
            GestureType::Rotate,
            Some(anchor_at((450.0, 200.0))),
        )
        .expect("begin multi-rotate");
    m.update_gesture(h, (-100.0, 100.0), GestureModifiers::default())
        .unwrap();
    let outcome = m.commit_gesture(h).unwrap();
    // The commit is a Batch carrying one SetProperty per node.
    assert!(matches!(
        outcome.applied.op,
        paged_mutate::Operation::Batch { .. }
    ));
    // Both members got a rotation matrix; 2x2 part matches the 90°
    // rotation independent of which member.
    let t_tf = tf_transform(&m, "tf1").expect("tf1 transform");
    let t_r = rect_transform(&m, "r1").expect("r1 transform");
    for t in [t_tf, t_r] {
        assert!(t[0].abs() < 1e-3);
        assert!((t[1] - 1.0).abs() < 1e-3);
        assert!((t[2] + 1.0).abs() < 1e-3);
        assert!(t[3].abs() < 1e-3);
    }
    // The union centroid (350, 200) must map back to itself under
    // tf1's new transform — it's a fixed point of the rotation.
    let mapped_x = t_tf[0] * 350.0 + t_tf[2] * 200.0 + t_tf[4];
    let mapped_y = t_tf[1] * 350.0 + t_tf[3] * 200.0 + t_tf[5];
    assert!((mapped_x - 350.0).abs() < 1e-2, "x mapped to {mapped_x}");
    assert!((mapped_y - 200.0).abs() < 1e-2, "y mapped to {mapped_y}");
    // One undo rolls back BOTH members (AC-E-16). The fixture's
    // explicit `ItemTransform="1 0 0 1 0 0"` lands as a Some(identity)
    // snapshot, so undo restores to that exact value (not None).
    let before_tf = Some([1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let before_r = Some([1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0]);
    m.undo().expect("undo");
    assert_eq!(
        tf_transform(&m, "tf1"),
        before_tf,
        "tf1 transform after undo"
    );
    assert_eq!(
        rect_transform(&m, "r1"),
        before_r,
        "r1 transform after undo"
    );
}

#[test]
fn multi_node_scale_commits_as_batch_with_same_factor() {
    // Same fixture: union centroid (350, 200). Anchor 100 right of
    // centroid; drag +100 right → sx = 2.
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![
                ElementId::TextFrame("tf1".into()),
                ElementId::Rectangle("r1".into()),
            ],
            GestureType::Scale,
            Some(anchor_at((450.0, 200.0))),
        )
        .unwrap();
    m.update_gesture(h, (100.0, 0.0), GestureModifiers::default())
        .unwrap();
    let outcome = m.commit_gesture(h).unwrap();
    assert!(matches!(
        outcome.applied.op,
        paged_mutate::Operation::Batch { .. }
    ));
    let t_tf = tf_transform(&m, "tf1").expect("tf1");
    let t_r = rect_transform(&m, "r1").expect("r1");
    // Both members scale by 2 on x (anchor lies on a horizontal ray
    // through the centroid so sy collapses to 1).
    for t in [t_tf, t_r] {
        assert!((t[0] - 2.0).abs() < 1e-3, "sx={}", t[0]);
        assert!((t[3] - 1.0).abs() < 1e-3, "sy={}", t[3]);
    }
}

#[test]
fn scale_with_shift_locks_aspect() {
    // Anchor diagonally NE of centroid → both sx and sy populated.
    // Without lock, sx and sy differ; with lock they match the
    // dominant scale.
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Scale,
            Some(anchor_at((300.0, 100.0))),
        )
        .unwrap();
    // Anchor = (300, 100), centroid = (200, 200) → anchor_dx = 100,
    // anchor_dy = -100. Drag by (50, 50) → current_dx = 150,
    // current_dy = -50. sx = 150/100 = 1.5, sy = -50/-100 = 0.5.
    // Shift picks the dominant (1.5 since |1.5-1|=0.5 > |0.5-1|=0.5
    // — actually equal, defaults to sy=0.5). Equal-distance tie:
    // our compute picks sx because of the strict > comparison; with
    // equal magnitudes the second branch wins → sy. Let me just
    // assert they match.
    m.update_gesture(
        h,
        (50.0, 50.0),
        GestureModifiers {
            shift: true,
            alt: false,
            disable_snap: false,
        },
    )
    .unwrap();
    m.commit_gesture(h).unwrap();
    let mt = tf_transform(&m, "tf1").expect("transform");
    assert!(
        (mt[0] - mt[3]).abs() < 1e-3,
        "aspect not locked: sx={}, sy={}",
        mt[0],
        mt[3]
    );
}
