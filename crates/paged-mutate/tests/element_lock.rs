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

//! C-77 — a locked page item is not moved, reshaped, restacked or
//! removed; its attributes still change, and unlocking it is an ordinary
//! property write. The rule is InDesign's, asked of it by script
//! (`tools/indesign-export/object-integrity-probe.json`, cases 1–4).
//! `elementLocked` used to be paint-neutral bookkeeping nothing read:
//! `moveFrame` and `deleteFrame` applied to a locked frame.

use std::path::PathBuf;

use paged_mutate::operation::{NodeSpec, PathAnchorSpec, ZOrderTarget};
use paged_mutate::{
    apply, GroupSpec, NodeId, Operation, OperationError, PathfinderKind, PropertyPath, Value,
};
use paged_scene::Document;
use serde_json::Value as Json;

fn fixture() -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry.idml");
    let bytes = std::fs::read(path).expect("read geometry fixture");
    idml_import::import_idml_doc(&bytes).expect("open")
}

fn snapshot(doc: &Document) -> Json {
    let spreads: Vec<Json> = doc
        .spreads
        .iter()
        .map(|p| serde_json::json!({ "src": p.src, "spread": p.spread }))
        .collect();
    serde_json::json!({ "spreads": spreads })
}

fn spread_id(doc: &Document) -> String {
    doc.spreads[0].spread.self_id.clone().expect("spread id")
}

fn add_rect(doc: &mut Document, id: &str, x: f32) -> NodeId {
    let parent = NodeId::Spread(spread_id(doc));
    let position = doc.spreads[0].spread.rectangles.len();
    apply(
        doc,
        &Operation::InsertNode {
            parent,
            position,
            z_slot: None,
            node: NodeSpec::Rectangle {
                self_id: id.to_string(),
                bounds: [100.0, x, 160.0, x + 40.0],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
        },
    )
    .unwrap_or_else(|e| panic!("insert {id}: {e:?}"));
    NodeId::Rectangle(id.to_string())
}

fn add_square_path(doc: &mut Document, id: &str, x: f32) -> NodeId {
    let parent = NodeId::Spread(spread_id(doc));
    let position = doc.spreads[0].spread.polygons.len();
    let anchors = [(x, 300.0), (x + 60.0, 300.0), (x + 60.0, 360.0), (x, 360.0)]
        .iter()
        .map(|&(x, y)| PathAnchorSpec {
            anchor: [x, y],
            left: [x, y],
            right: [x, y],
        })
        .collect();
    apply(
        doc,
        &Operation::InsertNode {
            parent,
            position,
            z_slot: None,
            node: NodeSpec::Polygon {
                self_id: id.to_string(),
                bounds: [300.0, x, 360.0, x + 60.0],
                anchors,
                subpath_starts: vec![0],
                subpath_open: vec![false],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
        },
    )
    .unwrap_or_else(|e| panic!("insert {id}: {e:?}"));
    NodeId::Polygon(id.to_string())
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

fn lock(doc: &mut Document, node: &NodeId, locked: bool) {
    apply(
        doc,
        &set(node, PropertyPath::ElementLocked, Value::Bool(locked)),
    )
    .unwrap_or_else(|e| panic!("lock {node:?} = {locked}: {e:?}"));
}

/// The op is refused as `Locked`, naming `who`, and nothing changed.
fn assert_refused(doc: &mut Document, op: Operation, who: &NodeId, what: &str) {
    let before = snapshot(doc);
    match apply(doc, &op) {
        Err(OperationError::Locked { node, .. }) => assert_eq!(&node, who, "{what}"),
        Err(OperationError::BatchFailed { source, .. })
            if matches!(*source, OperationError::Locked { .. }) =>
        {
            let OperationError::Locked { node, .. } = *source else {
                unreachable!()
            };
            assert_eq!(&node, who, "{what}");
        }
        other => panic!("{what}: expected a Locked refusal, got {other:?}"),
    }
    assert_eq!(snapshot(doc), before, "{what}: nothing changed");
}

fn group(doc: &mut Document, members: &[NodeId]) -> String {
    let applied = apply(
        doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: members.to_vec(),
                parent: None,
                item_transform: None,
                opacity: None,
                blend_mode: None,
            },
        },
    )
    .expect("create group");
    match applied.op {
        Operation::CreateGroup { spec } => spec.self_id.expect("minted group id"),
        other => panic!("unexpected echo {other:?}"),
    }
}

/// InDesign 1a–1e, 1k, 1m: move, resize, rotate, scale, reshape, restack
/// and remove all refuse on a locked item.
#[test]
fn a_locked_item_is_not_moved_reshaped_restacked_or_removed() {
    let mut doc = fixture();
    add_rect(&mut doc, "c77below", 40.0);
    let r = add_rect(&mut doc, "c77r", 100.0);
    lock(&mut doc, &r, true);

    let spread = NodeId::Spread(spread_id(&doc));
    let refused: Vec<(&str, Operation)> = vec![
        ("delete", Operation::RemoveNode { node: r.clone() }),
        (
            "move",
            set(
                &r,
                PropertyPath::FrameTransform,
                Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 30.0, 0.0])),
            ),
        ),
        (
            "resize",
            set(
                &r,
                PropertyPath::FrameBounds,
                Value::Bounds([100.0, 100.0, 200.0, 200.0]),
            ),
        ),
        (
            "rotate",
            set(
                &r,
                PropertyPath::FrameRotationAngle,
                Value::Length(Some(30.0)),
            ),
        ),
        (
            "scale",
            set(&r, PropertyPath::FrameScaleX, Value::Length(Some(200.0))),
        ),
        ("flip", set(&r, PropertyPath::FrameFlipH, Value::Bool(true))),
        (
            "move its content",
            set(
                &r,
                PropertyPath::ImageContentTransform,
                Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 5.0, 5.0])),
            ),
        ),
        (
            "send to back",
            Operation::ReorderNode {
                node: r.clone(),
                target: ZOrderTarget::Back,
            },
        ),
        (
            "move to another spread",
            Operation::MoveNode {
                node: r.clone(),
                new_parent: spread,
                position: 0,
            },
        ),
        (
            "drag a copy",
            Operation::InsertNode {
                parent: NodeId::Spread(spread_id(&doc)),
                position: 0,
                z_slot: None,
                node: NodeSpec::CloneTranslate {
                    self_id: "c77copy".to_string(),
                    source: r.clone(),
                    dx: 10.0,
                    dy: 10.0,
                    destination_spread_id: None,
                },
            },
        ),
    ];
    for (what, op) in refused {
        assert_refused(&mut doc, op, &r, what);
    }
}

/// InDesign 1e: a path-point write on a locked item changes nothing
/// there; here it is refused with the reason.
#[test]
fn a_locked_paths_points_are_not_moved() {
    let mut doc = fixture();
    let p = add_square_path(&mut doc, "c77p", 100.0);
    lock(&mut doc, &p, true);
    let anchors = [
        (90.0, 290.0),
        (160.0, 300.0),
        (160.0, 360.0),
        (100.0, 360.0),
    ]
    .iter()
    .map(|&(x, y)| PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    })
    .collect();
    assert_refused(
        &mut doc,
        set(
            &p,
            PropertyPath::FramePath,
            Value::FramePath {
                anchors,
                subpath_starts: vec![0],
            },
        ),
        &p,
        "reshape the path",
    );
}

/// InDesign 1f–1j, 2c/2f: the attributes of a locked item change.
#[test]
fn a_locked_items_attributes_still_change() {
    let mut doc = fixture();
    let r = add_rect(&mut doc, "c77attr", 100.0);
    lock(&mut doc, &r, true);
    for (path, value) in [
        (
            PropertyPath::FrameFillColor,
            Value::ColorRef(Some("Color/Paper".to_string())),
        ),
        (PropertyPath::FrameStrokeWeight, Value::Length(Some(4.0))),
        (PropertyPath::FrameOpacity, Value::Length(Some(40.0))),
        (
            PropertyPath::FrameCornerRadiusTopLeft,
            Value::Length(Some(6.0)),
        ),
        (PropertyPath::FrameNonprinting, Value::Bool(true)),
    ] {
        apply(&mut doc, &set(&r, path, value))
            .unwrap_or_else(|e| panic!("{path:?} on a locked item: {e:?}"));
    }
}

/// InDesign 1n–1p: unlocking is a property write, and then it moves
/// and goes.
#[test]
fn unlocking_is_a_property_write_and_then_it_moves_and_goes() {
    let mut doc = fixture();
    let r = add_rect(&mut doc, "c77u", 100.0);
    lock(&mut doc, &r, true);
    lock(&mut doc, &r, false);
    apply(
        &mut doc,
        &set(
            &r,
            PropertyPath::FrameTransform,
            Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 30.0, 0.0])),
        ),
    )
    .expect("moves once unlocked");
    apply(&mut doc, &Operation::RemoveNode { node: r }).expect("goes once unlocked");
}

/// InDesign 4d–4f: the content of a locked container is locked — it
/// does not move — and the locked container is not removed (with it).
#[test]
fn the_content_of_a_locked_container_is_locked_too() {
    let mut doc = fixture();
    let host = add_rect(&mut doc, "c77host", 100.0);
    let child = add_rect(&mut doc, "c77child", 110.0);
    apply(
        &mut doc,
        &Operation::PasteInto {
            container: host.clone(),
            child: child.clone(),
            child_index: None,
        },
    )
    .expect("paste into");
    lock(&mut doc, &host, true);
    assert!(paged_mutate::is_locked(&doc, &child));
    assert_refused(
        &mut doc,
        set(
            &child,
            PropertyPath::FrameTransform,
            Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 5.0, 5.0])),
        ),
        &child,
        "move the content",
    );
    assert_refused(
        &mut doc,
        Operation::RemoveNode { node: host.clone() },
        &host,
        "delete the container",
    );
}

/// A group holding a locked member does not move as a unit: that would
/// move the locked item. (InDesign cannot lock a member on its own, 3g;
/// the engine can, so it says which member stops the move.)
#[test]
fn a_group_holding_a_locked_member_does_not_move() {
    let mut doc = fixture();
    let a = add_rect(&mut doc, "c77ga", 100.0);
    let b = add_rect(&mut doc, "c77gb", 160.0);
    let g = group(&mut doc, &[a, b.clone()]);
    lock(&mut doc, &b, true);
    assert_refused(
        &mut doc,
        Operation::SetGroupTransform {
            group: g,
            transform: Some([1.0, 0.0, 0.0, 1.0, 20.0, 0.0]),
            prev: None,
        },
        &b,
        "move the group",
    );
}

/// The verbs that consume or reshape their inputs refuse a locked one.
#[test]
fn a_pathfinder_or_join_with_a_locked_input_is_refused() {
    let mut doc = fixture();
    let kept = add_square_path(&mut doc, "c77k", 100.0);
    let other = add_square_path(&mut doc, "c77o", 130.0);
    lock(&mut doc, &other, true);
    assert_refused(
        &mut doc,
        Operation::PathfinderBoolean {
            kept: kept.clone(),
            others: vec![other.clone()],
            op_kind: PathfinderKind::Union,
        },
        &other,
        "pathfinder",
    );
    assert_refused(
        &mut doc,
        Operation::JoinPaths {
            kept,
            other: other.clone(),
        },
        &other,
        "join",
    );
}

/// A batch with a refused child is refused whole.
#[test]
fn a_batch_that_moves_a_locked_item_is_refused_whole() {
    let mut doc = fixture();
    let free = add_rect(&mut doc, "c77free", 40.0);
    let r = add_rect(&mut doc, "c77b", 100.0);
    lock(&mut doc, &r, true);
    assert_refused(
        &mut doc,
        Operation::Batch {
            ops: vec![
                set(
                    &free,
                    PropertyPath::FrameTransform,
                    Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 30.0, 0.0])),
                ),
                set(
                    &r,
                    PropertyPath::FrameTransform,
                    Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 30.0, 0.0])),
                ),
            ],
        },
        &r,
        "batch",
    );
}

/// Undo is never refused: a lock set after a move is undone first.
#[test]
fn undo_runs_back_through_a_lock_set_after_a_move() {
    let mut doc = fixture();
    let r = add_rect(&mut doc, "c77undo", 100.0);
    let before = snapshot(&doc);
    let moved = apply(
        &mut doc,
        &set(
            &r,
            PropertyPath::FrameTransform,
            Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 30.0, 0.0])),
        ),
    )
    .expect("move");
    let locked = apply(
        &mut doc,
        &set(&r, PropertyPath::ElementLocked, Value::Bool(true)),
    )
    .expect("lock");
    apply(&mut doc, &locked.inverse).expect("undo the lock");
    apply(&mut doc, &moved.inverse).expect("undo the move");
    assert_eq!(snapshot(&doc), before);
}

/// InDesign 1l: a locked item can be duplicated, and the copy is locked
/// too; the duplicate's undo removes it.
#[test]
fn a_locked_item_can_be_duplicated_and_the_duplicate_undone() {
    let mut doc = fixture();
    let r = add_rect(&mut doc, "c77dup", 100.0);
    lock(&mut doc, &r, true);
    let before = snapshot(&doc);
    let applied = apply(
        &mut doc,
        &Operation::DuplicateNodes {
            sources: vec![r],
            dx: 10.0,
            dy: 10.0,
            ids: Vec::new(),
            story_ids: Vec::new(),
        },
    )
    .expect("duplicate a locked item");
    let copies: Vec<bool> = doc.spreads[0]
        .spread
        .rectangles
        .iter()
        .filter(|f| f.self_id.as_deref() != Some("c77dup"))
        .map(|f| f.locked)
        .collect();
    assert!(copies.contains(&true), "the copy is locked too");
    apply(&mut doc, &applied.inverse).expect("undo the duplicate");
    assert_eq!(snapshot(&doc), before);
}
