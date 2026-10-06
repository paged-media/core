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

//! C-74 — removing or inserting a page item keeps every group's member
//! table pointing at the items it named.
//!
//! A `Group` holds `members: Vec<FrameRef>` — INDICES into the spread's
//! per-kind vecs. Removing an item shifts every later item of its kind
//! down by one; `unregister_frame_ref` renumbered the z-table and the
//! pasted-in children for that and never touched `groups[..].members`,
//! so every group ref past the removed slot pointed one item along. The
//! op answered success. Four rectangles `r1..r4`, `group [r2, r3]`:
//!
//! ```text
//! before        r1, group[r2, r3], r4
//! remove r1     group[r3, r4], r4          (r2 gone, r4 twice)
//! ```
//!
//! `register_frame_ref` had the mirror gap on insert.
//!
//! Everything here reads the tables back BY ID, which is the only
//! reading that can see the fault: the indices themselves look fine.

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_model::FrameRef;
use paged_mutate::operation::{NodeSpec, PathAnchorSpec};
use paged_mutate::{apply, GroupSpec, NodeId, Operation, PathfinderKind};
use paged_renderer::pipeline::{build_document, PipelineOptions};
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

fn spread_id(doc: &Document) -> String {
    doc.spreads[0].spread.self_id.clone().expect("spread id")
}

/// Append a filled rectangle to the first spread, on top.
fn add_rect(doc: &mut Document, id: &str, x: f32) -> NodeId {
    let position = doc.spreads[0].spread.rectangles.len();
    insert_rect_at(doc, id, x, position);
    NodeId::Rectangle(id.to_string())
}

fn insert_rect_at(doc: &mut Document, id: &str, x: f32, position: usize) {
    let parent = NodeId::Spread(spread_id(doc));
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

fn id_of(spread: &paged_model::Spread, r: FrameRef) -> String {
    match r {
        FrameRef::TextFrame(i) => spread.text_frames.get(i).and_then(|f| f.self_id.clone()),
        FrameRef::Rectangle(i) => spread.rectangles.get(i).and_then(|f| f.self_id.clone()),
        FrameRef::Oval(i) => spread.ovals.get(i).and_then(|f| f.self_id.clone()),
        FrameRef::GraphicLine(i) => spread.graphic_lines.get(i).and_then(|f| f.self_id.clone()),
        FrameRef::Polygon(i) => spread.polygons.get(i).and_then(|f| f.self_id.clone()),
        FrameRef::Group(i) => spread.groups.get(i).and_then(|f| f.self_id.clone()),
    }
    .unwrap_or_else(|| format!("<dangling {r:?}>"))
}

/// A group's members, by id.
fn members(doc: &Document, group_id: &str) -> Vec<String> {
    let spread = &doc.spreads[0].spread;
    let g = spread
        .groups
        .iter()
        .find(|g| g.self_id.as_deref() == Some(group_id))
        .unwrap_or_else(|| panic!("group {group_id}"));
    g.members.iter().map(|r| id_of(spread, *r)).collect()
}

/// The spread's top-level items by id, in z-order.
fn top_level(doc: &Document) -> Vec<String> {
    let spread = &doc.spreads[0].spread;
    spread
        .frames_in_order
        .iter()
        .map(|r| id_of(spread, *r))
        .collect()
}

fn commands(doc: &Document) -> Vec<String> {
    build_document(doc, &PipelineOptions::default())
        .expect("build")
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter().map(|c| format!("{c:?}")))
        .collect()
}

/// `r1, group[r2, r3], r4` — the finding's own document.
fn staged() -> (Document, String) {
    let mut doc = fixture();
    add_rect(&mut doc, "c74r1", 100.0);
    let r2 = add_rect(&mut doc, "c74r2", 200.0);
    let r3 = add_rect(&mut doc, "c74r3", 300.0);
    add_rect(&mut doc, "c74r4", 400.0);
    let g = group(&mut doc, &[r2, r3]);
    (doc, g)
}

#[test]
fn removing_an_earlier_item_leaves_the_group_holding_its_own_members() {
    let (mut doc, g) = staged();
    assert_eq!(members(&doc, &g), ["c74r2", "c74r3"]);
    let before = snapshot(&doc);

    let applied = apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("c74r1".to_string()),
        },
    )
    .expect("remove r1");
    assert_eq!(
        members(&doc, &g),
        ["c74r2", "c74r3"],
        "the group still holds r2 and r3"
    );
    let tops = top_level(&doc);
    assert!(
        tops.contains(&"c74r4".to_string()) && !tops.contains(&"c74r1".to_string()),
        "r4 is still a top-level item and r1 is gone: {tops:?}"
    );
    assert_eq!(
        tops.iter().filter(|id| *id == "c74r4").count(),
        1,
        "r4 is listed once: {tops:?}"
    );

    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(snapshot(&doc), before, "undo restores the document exactly");
}

/// The item removed IS a member: it leaves its group, and undo puts it
/// back INTO the group at the slot it had — not at the top level.
#[test]
fn removing_a_member_takes_it_out_of_its_group_and_undo_puts_it_back_in() {
    let (mut doc, g) = staged();
    let before = snapshot(&doc);
    let painted = commands(&doc);

    let applied = apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("c74r2".to_string()),
        },
    )
    .expect("remove a member");
    assert_eq!(
        members(&doc, &g),
        ["c74r3"],
        "the group keeps its other member"
    );
    assert!(
        !top_level(&doc).contains(&"c74r3".to_string()),
        "…which is still inside it, not at the top level"
    );

    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(
        members(&doc, &g),
        ["c74r2", "c74r3"],
        "undo re-seats the member in its group, first as before"
    );
    assert!(
        !top_level(&doc).contains(&"c74r2".to_string()),
        "…and NOT as a second top-level entry"
    );
    assert_eq!(snapshot(&doc), before, "the document is exactly as it was");
    assert_eq!(commands(&doc), painted, "and paints as it did");

    apply(&mut doc, &undone.inverse).expect("redo");
    assert_eq!(members(&doc, &g), ["c74r3"]);
}

/// The mirror gap: an insert BELOW grouped items of its kind (what the
/// undo of a delete is) renumbered the z-table and not the members.
#[test]
fn inserting_below_grouped_items_leaves_the_group_holding_its_own_members() {
    let (mut doc, g) = staged();
    insert_rect_at(&mut doc, "c74first", 500.0, 0);
    assert_eq!(
        members(&doc, &g),
        ["c74r2", "c74r3"],
        "a rectangle inserted at the front of the kind vec moves no member"
    );
    let tops = top_level(&doc);
    for id in ["c74r1", "c74r4", "c74first"] {
        assert_eq!(
            tops.iter().filter(|t| *t == id).count(),
            1,
            "{id} is a top-level item, once: {tops:?}"
        );
    }
}

/// Removing a later item, or an item of another kind, was always clean;
/// pinned so the fix cannot trade one direction for the other.
#[test]
fn removing_a_later_item_or_another_kind_is_still_clean() {
    let (mut doc, g) = staged();
    apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("c74r4".to_string()),
        },
    )
    .expect("remove r4");
    assert_eq!(members(&doc, &g), ["c74r2", "c74r3"]);
    let text_frame = doc.spreads[0]
        .spread
        .text_frames
        .iter()
        .find_map(|f| f.self_id.clone())
        .expect("the fixture carries a text frame");
    apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::TextFrame(text_frame),
        },
    )
    .expect("remove a text frame");
    assert_eq!(members(&doc, &g), ["c74r2", "c74r3"]);
}

/// A nested group's members are members too: the outer group's
/// `FrameRef::Group` and the inner group's leaves both survive.
#[test]
fn a_nested_groups_members_survive_a_removal_below_them() {
    let (mut doc, inner) = staged();
    let r4 = NodeId::Rectangle("c74r4".to_string());
    let outer = group(&mut doc, &[NodeId::Group(inner.clone()), r4]);
    apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("c74r1".to_string()),
        },
    )
    .expect("remove r1");
    assert_eq!(members(&doc, &inner), ["c74r2", "c74r3"]);
    assert_eq!(members(&doc, &outer), [inner.as_str(), "c74r4"]);
}

/// `MoveNode` re-parents an item to another spread, and its inverse can
/// only name a spread: a member moved out could not be undone back INTO
/// its group. So the move is refused, by name, and writes nothing —
/// rather than applying and leaving the group one member short on undo.
#[test]
fn moving_a_member_to_another_spread_is_refused_by_name() {
    let (mut doc, g) = staged();
    assert!(doc.spreads.len() > 1, "the fixture carries a second spread");
    let other = doc.spreads[1].spread.self_id.clone().expect("spread id");
    let before = snapshot(&doc);
    let err = apply(
        &mut doc,
        &Operation::MoveNode {
            node: NodeId::Rectangle("c74r2".to_string()),
            new_parent: NodeId::Spread(other.clone()),
            position: 0,
        },
    )
    .expect_err("a member cannot be moved out from under its group");
    assert!(err.to_string().contains("member of a group"), "{err}");
    assert_eq!(snapshot(&doc), before, "the refusal writes nothing");
    assert_eq!(members(&doc, &g), ["c74r2", "c74r3"]);

    // A top-level item below the grouped ones still moves, and the
    // group it leaves behind keeps its members.
    apply(
        &mut doc,
        &Operation::MoveNode {
            node: NodeId::Rectangle("c74r1".to_string()),
            new_parent: NodeId::Spread(other),
            position: 0,
        },
    )
    .expect("a top-level item moves");
    assert_eq!(members(&doc, &g), ["c74r2", "c74r3"]);
}

/// A pathfinder verb removes its inputs through the same path. Two
/// polygons unite; a group of two LATER polygons must not be re-seated.
#[test]
fn a_pathfinder_that_removes_its_input_leaves_groups_alone() {
    let mut doc = fixture();
    let quad = |x: f32| -> Vec<PathAnchorSpec> {
        [(x, 300.0), (x + 60.0, 300.0), (x + 60.0, 360.0), (x, 360.0)]
            .iter()
            .map(|&(x, y)| PathAnchorSpec {
                anchor: [x, y],
                left: [x, y],
                right: [x, y],
            })
            .collect()
    };
    let add_poly = |doc: &mut Document, id: &str, x: f32| -> NodeId {
        let parent = NodeId::Spread(spread_id(doc));
        let position = doc.spreads[0].spread.polygons.len();
        apply(
            doc,
            &Operation::InsertNode {
                parent,
                position,
                z_slot: None,
                node: NodeSpec::Polygon {
                    self_id: id.to_string(),
                    bounds: [300.0, x, 360.0, x + 60.0],
                    anchors: quad(x),
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
    };
    // p1 and p2 overlap; p3 and p4 are grouped and created after them.
    let p1 = add_poly(&mut doc, "c74p1", 100.0);
    let p2 = add_poly(&mut doc, "c74p2", 130.0);
    let p3 = add_poly(&mut doc, "c74p3", 300.0);
    let p4 = add_poly(&mut doc, "c74p4", 400.0);
    let g = group(&mut doc, &[p3, p4]);
    let before = snapshot(&doc);

    // `kept` survives; the other input is removed — and it sits BELOW
    // the grouped polygons in the kind vec.
    let applied = apply(
        &mut doc,
        &Operation::PathfinderBoolean {
            kept: p2,
            others: vec![p1],
            op_kind: PathfinderKind::Union,
        },
    )
    .expect("union");
    assert_eq!(
        members(&doc, &g),
        ["c74p3", "c74p4"],
        "the union removed p1 and the group still holds p3 and p4"
    );
    apply(&mut doc, &applied.inverse).expect("undo the union");
    assert_eq!(
        snapshot(&doc),
        before,
        "undo restores both inputs and the group"
    );
}

/// What a corrupted member table paints: the group's bracket closes
/// around the wrong items. A group with opacity is the visible case —
/// after removing `r1` the SAME two rectangles must be inside it.
#[test]
fn the_groups_bracket_still_wraps_the_same_items_after_a_removal() {
    let (mut doc, g) = staged();
    apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Group(g),
            path: paged_mutate::PropertyPath::FrameOpacity,
            value: paged_mutate::Value::Length(Some(50.0)),
        },
    )
    .expect("group opacity");
    /// The commands inside the 50 % blend group, as text.
    fn inside_the_bracket(doc: &Document) -> Vec<String> {
        let built = build_document(doc, &PipelineOptions::default()).expect("build");
        let mut inside = Vec::new();
        let mut open = false;
        for c in built.pages.iter().flat_map(|p| p.list.commands.iter()) {
            match c {
                DisplayCommand::BeginBlendGroup { opacity, .. }
                    if (*opacity - 0.5).abs() < 1e-6 =>
                {
                    open = true;
                }
                DisplayCommand::EndBlendGroup(_) if open => open = false,
                other if open => inside.push(format!("{other:?}")),
                _ => {}
            }
        }
        inside
    }
    let before = inside_the_bracket(&doc);
    assert!(
        before.len() >= 2,
        "both members paint inside the bracket: {before:?}"
    );
    apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("c74r1".to_string()),
        },
    )
    .expect("remove r1");
    assert_eq!(
        inside_the_bracket(&doc),
        before,
        "the same members paint inside the bracket, where they did"
    );
}

/// The draw plugin's report: delete an item BELOW a group, then group
/// something else — the second grouping was refused with "a member
/// already belongs to another group", because the first group's member
/// table had slid onto items it never held. With the table renumbered
/// (C-74) the regroup is accepted and both groups hold what they were
/// given.
#[test]
fn grouping_again_after_deleting_an_item_below_a_group_is_accepted() {
    let (mut doc, g) = staged();
    apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("c74r1".to_string()),
        },
    )
    .expect("delete the item below the group");
    let r5 = add_rect(&mut doc, "c74r5", 500.0);
    let second = group(&mut doc, &[NodeId::Rectangle("c74r4".to_string()), r5]);
    assert_eq!(members(&doc, &g), ["c74r2", "c74r3"]);
    assert_eq!(members(&doc, &second), ["c74r4", "c74r5"]);
}
