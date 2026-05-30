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

//! Phase B/C/D — gesture spine integration tests (translate + resize
//! + rotate). The pure math is covered by `gesture::tests` in
//! `gesture.rs`; this file drives the full `begin / update / commit /
//! cancel` lifecycle against a real `CanvasModel`.

use std::io::Write;

use paged_canvas::{
    CanvasModel, CanvasOptions, ElementId, GestureModifiers, GestureType, ResizeHandle,
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
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf2" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="0 1 -1 0 200 200"/>
<Rectangle Self="r1" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello</Content></CharacterStyleRange>
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

fn tf_bounds(m: &CanvasModel, id: &str) -> [f32; 4] {
    let f = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some(id))
        .expect("frame found");
    [f.bounds.top, f.bounds.left, f.bounds.bottom, f.bounds.right]
}

fn tf_transform(m: &CanvasModel, id: &str) -> Option<[f32; 6]> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some(id))
        .and_then(|f| f.item_transform)
}

#[test]
fn translate_shifts_bounds_by_delta_after_commit() {
    let mut m = model();
    let before = tf_bounds(&m, "tf1");

    let handle = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Translate,
            None,
        )
        .expect("begin");
    let _ = m
        .update_gesture(handle, (15.0, 25.0), GestureModifiers::default())
        .expect("update");
    let outcome = m.commit_gesture(handle).expect("commit");
    assert!(outcome.applied_seq > 0);

    let after = tf_bounds(&m, "tf1");
    assert!((after[0] - (before[0] + 25.0)).abs() < 1e-3);
    assert!((after[1] - (before[1] + 15.0)).abs() < 1e-3);
    assert!((after[2] - (before[2] + 25.0)).abs() < 1e-3);
    assert!((after[3] - (before[3] + 15.0)).abs() < 1e-3);
    assert!(m.active_gesture_handle().is_none());
}

#[test]
fn cancel_gesture_restores_snapshot() {
    let mut m = model();
    let before = tf_bounds(&m, "tf1");

    let handle = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Translate,
            None,
        )
        .expect("begin");
    let _ = m
        .update_gesture(handle, (50.0, 50.0), GestureModifiers::default())
        .expect("update");
    let mid = tf_bounds(&m, "tf1");
    assert!((mid[0] - before[0] - 50.0).abs() < 1e-3);

    m.cancel_gesture(handle).expect("cancel");
    let after = tf_bounds(&m, "tf1");
    assert_eq!(before, after);
    assert!(m.active_gesture_handle().is_none());
}

#[test]
fn translate_undo_round_trip_restores_scene() {
    let mut m = model();
    let before = tf_bounds(&m, "tf1");

    let handle = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Translate,
            None,
        )
        .unwrap();
    m.update_gesture(handle, (10.0, 20.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(handle).unwrap();

    m.undo().expect("undo");
    let after = tf_bounds(&m, "tf1");
    assert_eq!(before, after);

    m.redo().expect("redo");
    let redone = tf_bounds(&m, "tf1");
    assert!((redone[0] - before[0] - 20.0).abs() < 1e-3);
}

#[test]
fn rotated_frame_translate_now_uses_frame_transform() {
    // Phase D — Phase B used to reject rotated frames here; that
    // restriction lifted once FrameTransform landed. The translate
    // routes through ItemTransform's tx/ty instead of the bounds path.
    let mut m = model();
    let before_t = tf_transform(&m, "tf2").expect("tf2 has transform");
    let handle = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf2".into())],
            GestureType::Translate,
            None,
        )
        .expect("rotated translate now allowed");
    m.update_gesture(handle, (12.0, -5.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(handle).unwrap();
    let after_t = tf_transform(&m, "tf2").expect("transform retained");
    // The 2x2 part stays unchanged; tx/ty shift by the delta.
    assert!((after_t[0] - before_t[0]).abs() < 1e-3);
    assert!((after_t[1] - before_t[1]).abs() < 1e-3);
    assert!((after_t[2] - before_t[2]).abs() < 1e-3);
    assert!((after_t[3] - before_t[3]).abs() < 1e-3);
    assert!((after_t[4] - before_t[4] - 12.0).abs() < 1e-3);
    assert!((after_t[5] - before_t[5] + 5.0).abs() < 1e-3);
}

#[test]
fn re_entrant_begin_returns_error() {
    let mut m = model();
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Translate,
            None,
        )
        .unwrap();
    let err = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Translate,
            None,
        )
        .expect_err("second begin must fail");
    assert!(format!("{err}").contains("already active"), "{err}");
    m.cancel_gesture(h).expect("cancel");
}

#[test]
fn multi_node_translate_commits_as_batch() {
    let mut m = model();
    let before_tf = tf_bounds(&m, "tf1");
    let before_r = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some("r1"))
        .map(|r| (r.bounds.top, r.bounds.left))
        .unwrap();

    let handle = m
        .begin_gesture(
            vec![
                ElementId::TextFrame("tf1".into()),
                ElementId::Rectangle("r1".into()),
            ],
            GestureType::Translate,
            None,
        )
        .unwrap();
    m.update_gesture(handle, (10.0, 10.0), GestureModifiers::default())
        .unwrap();
    let outcome = m.commit_gesture(handle).unwrap();
    assert!(matches!(
        outcome.applied.op,
        paged_mutate::Operation::Batch { .. }
    ));

    let after_tf = tf_bounds(&m, "tf1");
    assert!((after_tf[0] - before_tf[0] - 10.0).abs() < 1e-3);
    let after_r = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some("r1"))
        .map(|r| (r.bounds.top, r.bounds.left))
        .unwrap();
    assert!((after_r.0 - before_r.0 - 10.0).abs() < 1e-3);
    assert!((after_r.1 - before_r.1 - 10.0).abs() < 1e-3);

    m.undo().expect("undo");
    let restored_tf = tf_bounds(&m, "tf1");
    assert_eq!(restored_tf, before_tf);
}

#[test]
fn resize_se_handle_drives_through_lifecycle() {
    let mut m = model();
    let before = tf_bounds(&m, "tf1");
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Resize {
                handle: ResizeHandle::SouthEast,
            },
            None,
        )
        .expect("begin");
    m.update_gesture(h, (30.0, 40.0), GestureModifiers::default())
        .expect("update");
    m.commit_gesture(h).expect("commit");
    let after = tf_bounds(&m, "tf1");
    assert!((after[0] - before[0]).abs() < 1e-3, "top moved: {after:?}");
    assert!((after[1] - before[1]).abs() < 1e-3, "left moved");
    assert!((after[2] - before[2] - 40.0).abs() < 1e-3, "bottom delta");
    assert!((after[3] - before[3] - 30.0).abs() < 1e-3, "right delta");
}

#[test]
fn resize_undo_redo_round_trips() {
    let mut m = model();
    let before = tf_bounds(&m, "tf1");
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Resize {
                handle: ResizeHandle::East,
            },
            None,
        )
        .unwrap();
    m.update_gesture(h, (25.0, 0.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    m.undo().expect("undo");
    assert_eq!(before, tf_bounds(&m, "tf1"));
    m.redo().expect("redo");
    let redone = tf_bounds(&m, "tf1");
    assert!((redone[3] - before[3] - 25.0).abs() < 1e-3);
}

#[test]
fn resize_with_alt_keeps_centre_fixed() {
    let mut m = model();
    let before = tf_bounds(&m, "tf1");
    let cx_before = (before[1] + before[3]) * 0.5;
    let cy_before = (before[0] + before[2]) * 0.5;
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Resize {
                handle: ResizeHandle::SouthEast,
            },
            None,
        )
        .unwrap();
    m.update_gesture(
        h,
        (20.0, 10.0),
        GestureModifiers {
            shift: false,
            alt: true, disable_snap: false,
        },
    )
    .unwrap();
    m.commit_gesture(h).unwrap();
    let after = tf_bounds(&m, "tf1");
    let cx_after = (after[1] + after[3]) * 0.5;
    let cy_after = (after[0] + after[2]) * 0.5;
    assert!((cx_after - cx_before).abs() < 1e-3, "cx drifted");
    assert!((cy_after - cy_before).abs() < 1e-3, "cy drifted");
}

#[test]
fn rotated_frame_resize_now_works_in_local_coords() {
    // Phase G — Phase C used to reject rotated frames for Resize.
    // tf2 has a 90° rotation (`ItemTransform="0 1 -1 0 200 200"`).
    // Dragging the East handle by world-space (+30, 0) should land
    // as a content-box delta after inverse-rotation: (0, +30) — i.e.
    // the right edge of the local bounds moves outward by 30 in the
    // y direction of content-box space. Since the East handle moves
    // the `right` edge by dx_local, the right edge increases by
    // local_dx = world_dy (because the inverse rotation maps
    // (30, 0) → (0, 30)). Wait — the math:
    //   m = [0, 1, -1, 0, 200, 200] (90° rotation).
    //   inv linear = ((0, 1), (-1, 0)). Acting on world delta (30, 0):
    //     lx = 0*30 + 1*0  = 0
    //     ly = -1*30 + 0*0 = -30
    // So world (+30, 0) becomes local (0, -30). East handle moves
    // right by lx = 0 — no change. That's a bit confusing; let me
    // pick a delta that produces a clean local move.
    //
    // World (0, +30) → local: lx = 0*0 + 1*30 = 30; ly = -1*0 + 0*30 = 0.
    // So a downward world drag of 30pt becomes a local +30 in x → the
    // East handle moves the local right edge by +30.
    let mut m = model();
    let before = tf_bounds(&m, "tf2");
    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf2".into())],
            GestureType::Resize {
                handle: ResizeHandle::East,
            },
            None,
        )
        .expect("rotated resize now allowed");
    m.update_gesture(h, (0.0, 30.0), GestureModifiers::default())
        .unwrap();
    m.commit_gesture(h).unwrap();
    let after = tf_bounds(&m, "tf2");
    // Right edge moved by 30 in local coords; top/left/bottom unchanged.
    assert!((after[0] - before[0]).abs() < 1e-3, "top");
    assert!((after[1] - before[1]).abs() < 1e-3, "left");
    assert!((after[2] - before[2]).abs() < 1e-3, "bottom");
    assert!((after[3] - before[3] - 30.0).abs() < 1e-3, "right");
}

#[test]
fn alt_translate_duplicates_instead_of_moving() {
    // Phase H — Alt+Translate leaves the original at its snapshot
    // bounds and inserts a clone at (snapshot + delta).
    let mut m = model();
    let before_tf = tf_bounds(&m, "tf1");
    let original_count = m.scene().spreads[0].spread.text_frames.len();

    let h = m
        .begin_gesture(
            vec![ElementId::TextFrame("tf1".into())],
            GestureType::Translate,
            None,
        )
        .expect("begin");
    m.update_gesture(
        h,
        (15.0, 25.0),
        GestureModifiers {
            shift: false,
            alt: true, disable_snap: false,
        },
    )
    .expect("update");
    let outcome = m.commit_gesture(h).expect("commit");

    // Commit op is an InsertNode (or a Batch containing one).
    assert!(matches!(
        outcome.applied.op,
        paged_mutate::Operation::Batch { .. } | paged_mutate::Operation::InsertNode { .. }
    ));

    // Original tf1 unchanged.
    assert_eq!(tf_bounds(&m, "tf1"), before_tf);
    // One more text frame in the spread.
    assert_eq!(
        m.scene().spreads[0].spread.text_frames.len(),
        original_count + 1
    );
    // Find the duplicate — it has the same first 3 letters of the
    // source's id followed by "_dup_".
    let dup = m.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| {
            f.self_id
                .as_deref()
                .map_or(false, |s| s.starts_with("tf1_dup_"))
        })
        .expect("found duplicate");
    assert!((dup.bounds.top - before_tf[0] - 25.0).abs() < 1e-3);
    assert!((dup.bounds.left - before_tf[1] - 15.0).abs() < 1e-3);

    // Undo removes only the duplicate.
    m.undo().expect("undo");
    assert_eq!(
        m.scene().spreads[0].spread.text_frames.len(),
        original_count
    );
}

#[test]
fn determinism_replay_same_delta_lands_at_same_state() {
    let mut a = model();
    let mut b = model();
    for m in [&mut a, &mut b] {
        let h = m
            .begin_gesture(
                vec![ElementId::TextFrame("tf1".into())],
                GestureType::Translate,
                None,
            )
            .unwrap();
        m.update_gesture(h, (12.5, -7.25), GestureModifiers::default())
            .unwrap();
        m.commit_gesture(h).unwrap();
    }
    assert_eq!(tf_bounds(&a, "tf1"), tf_bounds(&b, "tf1"));
}
