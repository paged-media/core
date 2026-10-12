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

//! C-77 on the WIRE, as the editor's reproduction saw it
//! (`engine-findings.md` §13): `moveFrame` and `deleteFrame` applied to a
//! frame whose `elementLocked` is true. The rule (InDesign's, asked of it
//! by script: `tools/indesign-export/object-integrity-probe.json`) is the
//! engine's now, so a script or a plugin meets it as the editor does; the
//! kernel half is `paged-mutate/tests/element_lock.rs`.

use paged_canvas::{
    channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions, GestureError,
    GestureType, PageId,
};
use paged_mutate::{PropertyPath, Value};

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("geometry").expect("geometry sample");
    let bytes = paged_gen::write_idml(&sample).expect("write idml");
    CanvasModel::load("lock", &bytes, CanvasOptions::default()).expect("load")
}

fn page(m: &CanvasModel) -> PageId {
    PageId(m.pages()[0].self_id.clone())
}

fn frame(m: &mut CanvasModel, x: f32) -> ElementId {
    let page_id = page(m);
    m.apply_mutation(&Mutation::InsertFrame {
        page_id,
        bounds: (100.0, x, 160.0, x + 40.0),
    })
    .expect("insert frame")
    .created_id
    .expect("created id")
}

fn set(m: &mut CanvasModel, id: &ElementId, path: PropertyPath, value: Value) {
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path,
        value,
    })
    .unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
}

fn read(m: &CanvasModel, id: &ElementId, path: PropertyPath) -> Option<Value> {
    m.element_properties(id)
        .expect("props")
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .and_then(|e| e.value)
}

#[test]
fn moving_resizing_or_deleting_a_locked_frame_is_refused_and_says_why() {
    let mut m = model();
    let f = frame(&mut m, 100.0);
    set(&mut m, &f, PropertyPath::ElementLocked, Value::Bool(true));
    let bounds = read(&m, &f, PropertyPath::FrameBounds);
    let transform = read(&m, &f, PropertyPath::FrameTransform);
    let undo_depth = m.applied_log_len();

    let raw = f.raw_id().to_string();
    for (what, mutation) in [
        (
            "moveFrame",
            Mutation::MoveFrame {
                frame_id: raw.clone(),
                transform: [1.0, 0.0, 0.0, 1.0, 40.0, 20.0],
            },
        ),
        (
            "resizeFrame",
            Mutation::ResizeFrame {
                frame_id: raw.clone(),
                bounds: (100.0, 100.0, 200.0, 200.0),
            },
        ),
        (
            "deleteFrame",
            Mutation::DeleteFrame {
                frame_id: raw.clone(),
            },
        ),
    ] {
        let err = m
            .apply_mutation(&mutation)
            .expect_err(&format!("{what} on a locked frame"));
        let message = format!("{err:?}");
        assert!(
            message.contains("is locked"),
            "{what}: the refusal names the lock: {message}"
        );
    }
    assert_eq!(read(&m, &f, PropertyPath::FrameBounds), bounds);
    assert_eq!(read(&m, &f, PropertyPath::FrameTransform), transform);
    assert_eq!(m.applied_log_len(), undo_depth, "nothing was logged");
}

#[test]
fn a_locked_frame_takes_attribute_writes_and_moves_once_unlocked() {
    let mut m = model();
    let f = frame(&mut m, 100.0);
    set(&mut m, &f, PropertyPath::ElementLocked, Value::Bool(true));
    set(
        &mut m,
        &f,
        PropertyPath::FrameFillColor,
        Value::ColorRef(Some("Color/Paper".to_string())),
    );
    set(
        &mut m,
        &f,
        PropertyPath::FrameOpacity,
        Value::Length(Some(40.0)),
    );
    set(&mut m, &f, PropertyPath::ElementLocked, Value::Bool(false));
    m.apply_mutation(&Mutation::MoveFrame {
        frame_id: f.raw_id().to_string(),
        transform: [1.0, 0.0, 0.0, 1.0, 40.0, 20.0],
    })
    .expect("moves once unlocked");
}

/// The text in a locked text frame is edited normally (InDesign 2c/2d).
#[test]
fn text_in_a_locked_text_frame_is_edited() {
    let mut m = model();
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: page(&m),
            bounds: (300.0, 40.0, 400.0, 300.0),
        })
        .expect("text frame");
    let id = out.created_id.expect("created id");
    set(&mut m, &id, PropertyPath::ElementLocked, Value::Bool(true));
    let story = m.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(id.raw_id()))
        .and_then(|f| f.parent_story.clone())
        .expect("minted story");
    m.apply_mutation(&Mutation::InsertText {
        story_id: story,
        offset: 0,
        text: "locked frames take text".into(),
        cell: None,
    })
    .expect("text goes in");
}

/// The drag is refused before its preview moves anything: the commit
/// would be refused by the engine after the item had followed the
/// pointer.
#[test]
fn a_drag_on_a_locked_item_or_its_group_is_refused_at_begin() {
    let mut m = model();
    let a = frame(&mut m, 100.0);
    let b = frame(&mut m, 160.0);
    set(&mut m, &b, PropertyPath::ElementLocked, Value::Bool(true));

    for gesture in [GestureType::Translate, GestureType::Rotate] {
        match m.begin_gesture(vec![b.clone()], gesture, None) {
            Err(GestureError::Locked(id)) => assert_eq!(id, b),
            other => panic!("{gesture:?} on a locked frame: {other:?}"),
        }
    }

    let g = m
        .apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![a.clone(), b.clone()],
        })
        .expect("group")
        .created_id
        .expect("group id");
    match m.begin_gesture(vec![g], GestureType::Translate, None) {
        Err(GestureError::Locked(id)) => assert_eq!(id, b, "names the locked member"),
        other => panic!("dragging the group: {other:?}"),
    }

    // Unlocked, the same drag begins.
    set(&mut m, &b, PropertyPath::ElementLocked, Value::Bool(false));
    let handle = m
        .begin_gesture(vec![b], GestureType::Translate, None)
        .expect("begins once unlocked");
    m.cancel_gesture(handle).expect("cancel");
}
