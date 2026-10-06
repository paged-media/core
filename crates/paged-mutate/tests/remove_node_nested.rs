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

//! C-76 — deleting a container deletes what was pasted into it, and
//! undo restores the nesting.
//!
//! `pasteInto { container: u1, child: u2 }`, then `deleteFrame u1`: the
//! child stayed in its kind vec, named by a `nested_children` entry
//! whose host was gone, and the editor saw it come back as a free
//! top-level item; undo then gave `u1, u2`, both top-level (editor
//! engine-findings §12). InDesign deletes pasted-in content with its
//! frame, and so does this now: the children leave with the container
//! and come back INSIDE it, at their own slots, on undo.

use std::path::PathBuf;

use paged_model::FrameRef;
use paged_mutate::operation::NodeSpec;
use paged_mutate::{apply, NodeId, Operation};
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

/// Order-independent whole-document value (see `duplicate_nodes.rs`).
fn snapshot(doc: &Document) -> Json {
    let spreads: Vec<Json> = doc
        .spreads
        .iter()
        .map(|p| serde_json::json!({ "src": p.src, "spread": p.spread }))
        .collect();
    serde_json::json!({ "spreads": spreads })
}

fn add_rect(doc: &mut Document, id: &str, bounds: [f32; 4]) -> NodeId {
    let spread = &doc.spreads[0].spread;
    let parent = NodeId::Spread(spread.self_id.clone().expect("spread id"));
    let position = spread.rectangles.len();
    apply(
        doc,
        &Operation::InsertNode {
            parent,
            position,
            z_slot: None,
            node: NodeSpec::Rectangle {
                self_id: id.to_string(),
                bounds,
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

fn paste(doc: &mut Document, container: &NodeId, child: &NodeId) {
    apply(
        doc,
        &Operation::PasteInto {
            container: container.clone(),
            child: child.clone(),
            child_index: None,
        },
    )
    .expect("paste into");
}

fn exists(doc: &Document, id: &str) -> bool {
    doc.spreads.iter().any(|p| {
        p.spread
            .rectangles
            .iter()
            .any(|r| r.self_id.as_deref() == Some(id))
    })
}

/// The pasted-in children of `host`, by id.
fn children(doc: &Document, host: &str) -> Vec<String> {
    let spread = &doc.spreads[0].spread;
    spread
        .nested_children
        .get(host)
        .map(|v| {
            v.iter()
                .map(|r| match r {
                    FrameRef::Rectangle(i) => spread.rectangles[*i].self_id.clone().unwrap(),
                    other => panic!("unexpected child {other:?}"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn top_level(doc: &Document, id: &str) -> bool {
    let spread = &doc.spreads[0].spread;
    spread.frames_in_order.iter().any(|r| match r {
        FrameRef::Rectangle(i) => spread.rectangles[*i].self_id.as_deref() == Some(id),
        _ => false,
    })
}

#[test]
fn deleting_a_container_deletes_its_children_and_undo_renests_them() {
    let mut doc = fixture();
    let host = add_rect(&mut doc, "c76host", [100.0, 100.0, 300.0, 300.0]);
    let a = add_rect(&mut doc, "c76a", [120.0, 120.0, 180.0, 180.0]);
    let b = add_rect(&mut doc, "c76b", [200.0, 200.0, 260.0, 260.0]);
    let above = add_rect(&mut doc, "c76above", [400.0, 400.0, 420.0, 420.0]);
    paste(&mut doc, &host, &a);
    paste(&mut doc, &host, &b);
    assert_eq!(children(&doc, "c76host"), ["c76a", "c76b"]);
    let before = snapshot(&doc);

    let applied = apply(&mut doc, &Operation::RemoveNode { node: host.clone() })
        .expect("delete the container");
    for id in ["c76host", "c76a", "c76b"] {
        assert!(!exists(&doc, id), "{id} is gone with its container");
    }
    assert!(exists(&doc, "c76above") && top_level(&doc, "c76above"));
    assert!(
        doc.spreads[0].spread.nested_children.is_empty(),
        "no orphaned child list is left behind"
    );

    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(
        children(&doc, "c76host"),
        ["c76a", "c76b"],
        "undo re-nests both children, in order"
    );
    assert!(!top_level(&doc, "c76a") && !top_level(&doc, "c76b"));
    assert_eq!(snapshot(&doc), before, "undo restores the document exactly");

    apply(&mut doc, &undone.inverse).expect("redo");
    for id in ["c76host", "c76a", "c76b"] {
        assert!(!exists(&doc, id), "redo deletes {id} again");
    }
    let _ = above;
}

/// A container inside a container: the whole subtree goes, and comes
/// back nested at every level.
#[test]
fn a_nested_container_goes_and_returns_with_its_own_children() {
    let mut doc = fixture();
    let outer = add_rect(&mut doc, "c76outer", [100.0, 100.0, 400.0, 400.0]);
    let inner = add_rect(&mut doc, "c76inner", [150.0, 150.0, 300.0, 300.0]);
    let leaf = add_rect(&mut doc, "c76leaf", [180.0, 180.0, 220.0, 220.0]);
    paste(&mut doc, &inner, &leaf);
    paste(&mut doc, &outer, &inner);
    let before = snapshot(&doc);

    let applied = apply(&mut doc, &Operation::RemoveNode { node: outer }).expect("delete");
    for id in ["c76outer", "c76inner", "c76leaf"] {
        assert!(!exists(&doc, id), "{id} is gone");
    }
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(children(&doc, "c76outer"), ["c76inner"]);
    assert_eq!(children(&doc, "c76inner"), ["c76leaf"]);
    assert_eq!(snapshot(&doc), before);
}

/// Deleting the CHILD directly is still refused, legibly — that was
/// always right.
#[test]
fn deleting_a_pasted_in_child_directly_is_still_refused() {
    let mut doc = fixture();
    let host = add_rect(&mut doc, "c76host", [100.0, 100.0, 300.0, 300.0]);
    let a = add_rect(&mut doc, "c76a", [120.0, 120.0, 180.0, 180.0]);
    paste(&mut doc, &host, &a);
    let err = apply(&mut doc, &Operation::RemoveNode { node: a }).expect_err("refused");
    assert!(
        err.to_string().contains("release it before removing"),
        "{err}"
    );
}
