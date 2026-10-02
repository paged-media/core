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

//! C-64 — `Operation::DuplicateNodes`.
//!
//! A duplicate is the WHOLE item or a refusal, never a part of one. What
//! is pinned here, per kind:
//!
//! * the clone equals its source in every field but its id and its
//!   position (compared as whole serialised nodes, so a field the model
//!   gains later is covered without touching this file);
//! * it sits directly above its source in the list that names the
//!   source;
//! * undo removes exactly what was minted and leaves the document as it
//!   was, and redo brings the clones back under the same ids;
//! * the things a plain copy cannot be are refused by name, and a
//!   refusal writes nothing.

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_model::FrameRef;
use paged_mutate::operation::{NodeSpec, PathAnchorSpec};
use paged_mutate::{
    apply, duplicate_demand, duplicate_roots, DuplicateDemand, GroupSpec, NodeId, OpacityMaskMode,
    Operation, OperationError, PropertyPath, Value,
};
use paged_renderer::pipeline::{build_document, PipelineOptions};
use paged_scene::Document;
use serde_json::Value as Json;

fn fixture(name: &str) -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join(format!("{name}.idml"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    idml_import::import_idml_doc(&bytes).expect("open")
}

/// The whole document as a value: every spread and every story. Two
/// equal snapshots are the "nothing changed" this file means.
///
/// JSON rather than `{:?}`: the spread's side maps are `HashMap`s, and a
/// map that grew and shrank back can iterate in a different order than
/// it did before — a `Debug` string would call that a difference.
/// `serde_json`'s object is ordered by key.
fn snapshot(doc: &Document) -> Json {
    let spreads: Vec<Json> = doc
        .spreads
        .iter()
        .map(|p| serde_json::json!({ "src": p.src, "spread": p.spread }))
        .collect();
    let stories: Vec<Json> = doc
        .stories
        .iter()
        .map(|s| serde_json::json!({ "id": s.self_id, "src": s.src, "story": s.story }))
        .collect();
    serde_json::json!({ "spreads": spreads, "stories": stories })
}

fn duplicate(sources: &[NodeId], dx: f32, dy: f32) -> Operation {
    Operation::DuplicateNodes {
        sources: sources.to_vec(),
        dx,
        dy,
        ids: Vec::new(),
        story_ids: Vec::new(),
    }
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

fn corner(x: f32, y: f32) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    }
}

/// Append a pen path (a closed quad) to the first spread; returns it.
fn add_path(doc: &mut Document, id: &str) -> NodeId {
    let spread = &doc.spreads[0].spread;
    let parent = NodeId::Spread(spread.self_id.clone().expect("spread id"));
    let position = spread.polygons.len();
    apply(
        doc,
        &Operation::InsertNode {
            parent,
            position,
            z_slot: None,
            node: NodeSpec::Polygon {
                self_id: id.to_string(),
                bounds: [100.0, 100.0, 200.0, 300.0],
                anchors: vec![
                    corner(100.0, 100.0),
                    corner(300.0, 100.0),
                    corner(300.0, 200.0),
                    corner(100.0, 200.0),
                ],
                subpath_starts: vec![0],
                subpath_open: vec![false],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(2.0),
                item_transform: None,
            },
        },
    )
    .expect("insert path");
    NodeId::Polygon(id.to_string())
}

/// Append a filled rectangle to the first spread; returns it.
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
    .expect("insert rectangle");
    NodeId::Rectangle(id.to_string())
}

/// `(spread index, ref)` of a page item, by id.
fn locate(doc: &Document, id: &str) -> (usize, FrameRef) {
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let s = &parsed.spread;
        let at = |ids: Vec<Option<&str>>| ids.iter().position(|x| *x == Some(id));
        if let Some(i) = at(s.text_frames.iter().map(|f| f.self_id.as_deref()).collect()) {
            return (si, FrameRef::TextFrame(i));
        }
        if let Some(i) = at(s.rectangles.iter().map(|f| f.self_id.as_deref()).collect()) {
            return (si, FrameRef::Rectangle(i));
        }
        if let Some(i) = at(s.ovals.iter().map(|f| f.self_id.as_deref()).collect()) {
            return (si, FrameRef::Oval(i));
        }
        if let Some(i) = at(s
            .graphic_lines
            .iter()
            .map(|f| f.self_id.as_deref())
            .collect())
        {
            return (si, FrameRef::GraphicLine(i));
        }
        if let Some(i) = at(s.polygons.iter().map(|f| f.self_id.as_deref()).collect()) {
            return (si, FrameRef::Polygon(i));
        }
        if let Some(i) = at(s.groups.iter().map(|f| f.self_id.as_deref()).collect()) {
            return (si, FrameRef::Group(i));
        }
    }
    panic!("no page item {id:?}");
}

/// A page item, whole, as JSON.
fn node_json(doc: &Document, id: &str) -> Json {
    let (si, r) = locate(doc, id);
    let s = &doc.spreads[si].spread;
    match r {
        FrameRef::TextFrame(i) => serde_json::to_value(&s.text_frames[i]),
        FrameRef::Rectangle(i) => serde_json::to_value(&s.rectangles[i]),
        FrameRef::Oval(i) => serde_json::to_value(&s.ovals[i]),
        FrameRef::GraphicLine(i) => serde_json::to_value(&s.graphic_lines[i]),
        FrameRef::Polygon(i) => serde_json::to_value(&s.polygons[i]),
        FrameRef::Group(i) => serde_json::to_value(&s.groups[i]),
    }
    .expect("a model node serialises")
}

/// The ids an applied duplicate minted, and the story ids.
fn minted(applied: &paged_mutate::AppliedOperation) -> (Vec<String>, Vec<String>) {
    match &applied.op {
        Operation::DuplicateNodes { ids, story_ids, .. } => (ids.clone(), story_ids.clone()),
        other => panic!("expected the duplicate echoed, got {other:?}"),
    }
}

fn translation(json: &Json) -> (f64, f64) {
    match json.get("item_transform") {
        Some(Json::Array(m)) => (m[4].as_f64().unwrap(), m[5].as_f64().unwrap()),
        _ => (0.0, 0.0),
    }
}

/// `clone` equals `source` in everything but its id and where it sits.
fn assert_same_node_moved(doc: &Document, source: &str, clone: &str, dx: f64, dy: f64) {
    let src = node_json(doc, source);
    let mut cl = node_json(doc, clone);
    assert_eq!(cl["self_id"], Json::String(clone.to_string()));
    let (sx, sy) = translation(&src);
    let (cx, cy) = translation(&cl);
    assert!(
        (cx - sx - dx).abs() < 1e-3 && (cy - sy - dy).abs() < 1e-3,
        "{clone} sits ({dx}, {dy}) from {source}: {:?} vs {:?}",
        (cx, cy),
        (sx, sy)
    );
    cl["self_id"] = src["self_id"].clone();
    cl["item_transform"] = src["item_transform"].clone();
    // A group's members are refs to ITS OWN clones, and a text frame's
    // story is its own copy — compared separately, where they matter.
    for key in ["members", "parent_story"] {
        if src.get(key).is_some() {
            cl[key] = src[key].clone();
        }
    }
    let differing: Vec<&String> = src
        .as_object()
        .expect("a node is an object")
        .keys()
        .filter(|k| src[k.as_str()] != cl[k.as_str()])
        .collect();
    assert!(
        differing.is_empty(),
        "{clone} must be {source} in every other field; these differ: {differing:?}"
    );
    assert_eq!(cl, src, "…and carries no field its source lacks");
}

// ── a drawn path ────────────────────────────────────────────────────

#[test]
fn a_pen_path_duplicates_whole_directly_above_its_source() {
    let mut doc = fixture("geometry-groups");
    let node = add_path(&mut doc, "c64path");
    // Dress it with what the old half-clone dropped or never reached.
    for (path, value) in [
        (PropertyPath::FrameOpacity, Value::Length(Some(40.0))),
        (PropertyPath::FrameBlendMode, Value::Text("Multiply".into())),
        (PropertyPath::FrameOuterGlowEnabled, Value::Bool(true)),
        (
            PropertyPath::FrameStrokeAlignment,
            Value::Text("InsideAlignment".into()),
        ),
        (
            PropertyPath::FrameCornerRadiusTopLeft,
            Value::Length(Some(12.0)),
        ),
        (
            PropertyPath::PluginMetadata,
            Value::PluginMetadata {
                key: "x-paged:c64".to_string(),
                value: Some(r#"{"v":1,"data":{"k":"v"}}"#.to_string()),
                caller: None,
                prev: None,
            },
        ),
    ] {
        apply(&mut doc, &set(&node, path, value))
            .unwrap_or_else(|e| panic!("setup {path:?}: {e:?}"));
    }
    // Something drawn above it, so "directly above the source" is not
    // the same slot as "on top".
    add_rect(&mut doc, "c64above", [0.0, 0.0, 20.0, 20.0]);
    let before = snapshot(&doc);
    let fills_before = fill_count(&doc);

    let applied = apply(
        &mut doc,
        &duplicate(std::slice::from_ref(&node), 10.0, 20.0),
    )
    .expect("a pen path duplicates");
    let (ids, story_ids) = minted(&applied);
    assert_eq!((ids.len(), story_ids.len()), (1, 0), "one item, no story");
    let clone = ids[0].clone();
    assert_ne!(clone, "c64path");
    assert_eq!(
        duplicate_roots(&doc, &applied.op),
        vec![NodeId::Polygon(clone.clone())]
    );

    assert_same_node_moved(&doc, "c64path", &clone, 10.0, 20.0);
    let spread = &doc.spreads[0].spread;
    assert_eq!(
        spread.labels.get(&clone),
        spread.labels.get("c64path"),
        "the clone carries its source's labels"
    );
    assert!(spread.labels.contains_key(&clone), "…and there was one");
    // Directly above: the clone's z slot is the source's + 1, and the
    // rectangle drawn later is still above both.
    let z = |id: &str| {
        let (_, r) = locate(&doc, id);
        spread
            .frames_in_order
            .iter()
            .position(|x| *x == r)
            .unwrap_or_else(|| panic!("{id} is in the z-order"))
    };
    assert_eq!(z(&clone), z("c64path") + 1, "directly above its source");
    assert!(z("c64above") > z(&clone), "what was above stays above");
    assert_eq!(fill_count(&doc), fills_before + 1, "the clone is painted");

    // Undo: exactly as it was. Redo: the same clone, under the same id.
    let after = snapshot(&doc);
    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(snapshot(&doc), before, "undo leaves no trace");
    assert_eq!(fill_count(&doc), fills_before);
    apply(&mut doc, &undone.inverse).expect("redo");
    assert_eq!(snapshot(&doc), after, "redo reproduces, ids included");
}

fn fill_count(doc: &Document) -> usize {
    build_document(doc, &PipelineOptions::default())
        .expect("build")
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter())
        .filter(|c| {
            matches!(
                c,
                DisplayCommand::FillPath { .. } | DisplayCommand::FillPathBlend { .. }
            )
        })
        .count()
}

#[test]
fn every_leaf_kind_duplicates() {
    let mut doc = fixture("paste-into");
    let spread_id = doc.spreads[0].spread.self_id.clone().expect("spread id");
    let position = doc.spreads[0].spread.graphic_lines.len();
    apply(
        &mut doc,
        &Operation::InsertNode {
            parent: NodeId::Spread(spread_id),
            position,
            z_slot: None,
            node: NodeSpec::GraphicLine {
                self_id: "c64line".to_string(),
                bounds: [40.0, 40.0, 140.0, 140.0],
                anchors: vec![corner(40.0, 40.0), corner(140.0, 140.0)],
                subpath_starts: vec![],
                subpath_open: vec![],
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(1.0),
                item_transform: None,
            },
        },
    )
    .expect("insert line");
    let path = add_path(&mut doc, "c64path");
    let oval = doc.spreads[0]
        .spread
        .ovals
        .iter()
        .find_map(|o| o.self_id.clone())
        .map(NodeId::Oval)
        .expect("the fixture carries an oval");
    let sources = vec![NodeId::GraphicLine("c64line".to_string()), path, oval];
    let applied = apply(&mut doc, &duplicate(&sources, -5.0, 7.5)).expect("duplicate");
    let (ids, _) = minted(&applied);
    assert_eq!(ids.len(), 3);
    let roots = duplicate_roots(&doc, &applied.op);
    assert!(matches!(roots[0], NodeId::GraphicLine(_)));
    assert!(matches!(roots[1], NodeId::Polygon(_)));
    assert!(matches!(roots[2], NodeId::Oval(_)));
    for (source, clone) in sources.iter().zip(&ids) {
        assert_same_node_moved(&doc, source.self_id(), clone, -5.0, 7.5);
    }
}

// ── groups ──────────────────────────────────────────────────────────

/// Every id in the subtree at `id`, pre-order.
fn subtree(doc: &Document, id: &str) -> Vec<String> {
    let (si, r) = locate(doc, id);
    let spread = &doc.spreads[si].spread;
    let mut out = vec![id.to_string()];
    let child_id = |r: FrameRef| -> String {
        match r {
            FrameRef::TextFrame(i) => spread.text_frames[i].self_id.clone(),
            FrameRef::Rectangle(i) => spread.rectangles[i].self_id.clone(),
            FrameRef::Oval(i) => spread.ovals[i].self_id.clone(),
            FrameRef::GraphicLine(i) => spread.graphic_lines[i].self_id.clone(),
            FrameRef::Polygon(i) => spread.polygons[i].self_id.clone(),
            FrameRef::Group(i) => spread.groups[i].self_id.clone(),
        }
        .expect("a member with an id")
    };
    if let FrameRef::Group(gi) = r {
        for m in &spread.groups[gi].members {
            out.extend(subtree(doc, &child_id(*m)));
        }
    }
    if let Some(children) = spread.nested_children.get(id) {
        for c in children {
            out.extend(subtree(doc, &child_id(*c)));
        }
    }
    out
}

#[test]
fn a_group_duplicates_with_every_member_and_nested_group() {
    let mut doc = fixture("nested-groups");
    // The outermost group with a group among its members.
    let (group_id, members) = doc
        .spreads
        .iter()
        .flat_map(|p| p.spread.groups.iter())
        .filter(|g| g.members.iter().any(|m| matches!(m, FrameRef::Group(_))))
        .filter_map(|g| g.self_id.clone())
        .map(|id| {
            let n = subtree(&doc, &id).len();
            (id, n)
        })
        .max_by_key(|(_, n)| *n)
        .expect("nested-groups carries a group of groups");
    assert!(members >= 4, "a real nested structure: {members} nodes");
    let node = NodeId::Group(group_id.clone());
    assert_eq!(
        duplicate_demand(&doc, std::slice::from_ref(&node)).expect("demand"),
        DuplicateDemand {
            items: members,
            stories: 0
        }
    );
    // Give the group something of its own to carry across.
    apply(
        &mut doc,
        &set(&node, PropertyPath::FrameOpacity, Value::Length(Some(55.0))),
    )
    .expect("group opacity");
    let before = snapshot(&doc);
    let source_tree = subtree(&doc, &group_id);

    let applied = apply(&mut doc, &duplicate(std::slice::from_ref(&node), 30.0, 0.0))
        .expect("a group duplicates");
    let (ids, _) = minted(&applied);
    assert_eq!(ids.len(), members, "one id per node in the subtree");
    let clone = ids[0].clone();
    assert_eq!(
        subtree(&doc, &clone),
        ids,
        "the clone's subtree is the minted ids, in clone order"
    );
    // Node for node the same, each moved by the offset.
    for (s, c) in source_tree.iter().zip(&ids) {
        assert_same_node_moved(&doc, s, c, 30.0, 0.0);
    }
    // No clone shares a member with its source, and members stay out of
    // the z-order — only the group's own ref is listed, right above the
    // source group's.
    let (si, clone_ref) = locate(&doc, &clone);
    let (_, source_ref) = locate(&doc, &group_id);
    let spread = &doc.spreads[si].spread;
    let z = |r: FrameRef| spread.frames_in_order.iter().position(|x| *x == r);
    assert_eq!(z(clone_ref), z(source_ref).map(|i| i + 1));
    for id in &ids[1..] {
        let (_, r) = locate(&doc, id);
        assert_eq!(z(r), None, "{id} is a member, not a top-level item");
    }
    // The renderer brackets inner groups first by walking `groups` in
    // reverse, so a nested group must sit BEFORE its outer in the vec.
    for (gi, g) in spread.groups.iter().enumerate() {
        for m in &g.members {
            if let FrameRef::Group(inner) = m {
                assert!(*inner < gi, "nested group {inner} precedes its outer {gi}");
            }
        }
    }

    let after = snapshot(&doc);
    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(snapshot(&doc), before, "undo removes the whole subtree");
    apply(&mut doc, &undone.inverse).expect("redo");
    assert_eq!(snapshot(&doc), after, "redo reproduces the subtree");
}

#[test]
fn a_member_duplicated_inside_its_group_stays_in_that_group() {
    let mut doc = fixture("geometry-groups");
    let a = add_rect(&mut doc, "c64a", [100.0, 100.0, 300.0, 300.0]);
    let b = add_rect(&mut doc, "c64b", [150.0, 150.0, 350.0, 350.0]);
    let grouped = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![a.clone(), b],
                parent: None,
                item_transform: None,
                opacity: None,
                blend_mode: None,
            },
        },
    )
    .expect("group");
    let Operation::CreateGroup { spec } = grouped.op else {
        panic!("group echo");
    };
    let group_id = spec.self_id.expect("minted group id");
    let before = snapshot(&doc);

    let applied = apply(&mut doc, &duplicate(&[a], 5.0, 5.0)).expect("duplicate a member");
    let (ids, _) = minted(&applied);
    assert_eq!(
        subtree(&doc, &group_id),
        vec![
            group_id.clone(),
            "c64a".to_string(),
            ids[0].clone(),
            "c64b".to_string()
        ],
        "the clone is a member of the same group, right after its source"
    );
    let (si, r) = locate(&doc, &ids[0]);
    assert!(
        !doc.spreads[si].spread.frames_in_order.contains(&r),
        "…and not a top-level item"
    );
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(snapshot(&doc), before);
}

// ── containers ──────────────────────────────────────────────────────

#[test]
fn a_container_duplicates_with_its_pasted_in_children() {
    let mut doc = fixture("paste-into");
    let (host, children) = doc
        .spreads
        .iter()
        .flat_map(|p| p.spread.nested_children.iter())
        .map(|(host, kids)| (host.clone(), kids.len()))
        .next()
        .expect("paste-into carries a container with a pasted-in child");
    assert!(children >= 1);
    let (_, host_ref) = locate(&doc, &host);
    let node = match host_ref {
        FrameRef::Rectangle(_) => NodeId::Rectangle(host.clone()),
        FrameRef::Oval(_) => NodeId::Oval(host.clone()),
        FrameRef::Polygon(_) => NodeId::Polygon(host.clone()),
        other => panic!("unexpected container kind {other:?}"),
    };
    let before = snapshot(&doc);
    let source_tree = subtree(&doc, &host);

    let applied = apply(&mut doc, &duplicate(std::slice::from_ref(&node), 0.0, 40.0))
        .expect("a container duplicates");
    let (ids, _) = minted(&applied);
    assert_eq!(ids.len(), 1 + children, "the container and its children");
    assert_eq!(
        subtree(&doc, &ids[0]),
        ids,
        "the clone hosts its own children"
    );
    for (s, c) in source_tree.iter().zip(&ids) {
        assert_same_node_moved(&doc, s, c, 0.0, 40.0);
    }
    let spread = &doc.spreads[0].spread;
    assert_eq!(
        spread.nested_children[&host].len(),
        children,
        "the source keeps its own children, untouched"
    );
    // A pasted-in child is reached only through its container.
    for id in &ids[1..] {
        let (_, r) = locate(&doc, id);
        assert!(!spread.frames_in_order.contains(&r));
    }

    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(snapshot(&doc), before, "undo removes the children too");
}

// ── text frames ─────────────────────────────────────────────────────

/// An unthreaded text frame whose story is plain text.
fn plain_text_frame(doc: &Document) -> (String, String) {
    doc.spreads
        .iter()
        .flat_map(|p| p.spread.text_frames.iter())
        .find_map(|f| {
            let id = f.self_id.clone()?;
            let story = f.parent_story.clone()?;
            duplicate_demand(doc, &[NodeId::TextFrame(id.clone())])
                .is_ok()
                .then_some((id, story))
        })
        .expect("the fixture carries a plain, unthreaded text frame")
}

fn story_json(doc: &Document, id: &str) -> Json {
    serde_json::to_value(
        &doc.stories
            .iter()
            .find(|s| s.self_id == id)
            .unwrap_or_else(|| panic!("story {id}"))
            .story,
    )
    .expect("a story serialises")
}

#[test]
fn a_text_frame_duplicates_with_a_copy_of_its_story() {
    let mut doc = fixture("text");
    let (frame, story) = plain_text_frame(&doc);
    let node = NodeId::TextFrame(frame.clone());
    assert_eq!(
        duplicate_demand(&doc, std::slice::from_ref(&node)).expect("demand"),
        DuplicateDemand {
            items: 1,
            stories: 1
        }
    );
    let before = snapshot(&doc);
    let stories_before = doc.stories.len();

    let applied = apply(
        &mut doc,
        &duplicate(std::slice::from_ref(&node), 12.0, 12.0),
    )
    .expect("a plain text frame duplicates");
    let (ids, story_ids) = minted(&applied);
    assert_eq!((ids.len(), story_ids.len()), (1, 1));
    assert_same_node_moved(&doc, &frame, &ids[0], 12.0, 12.0);
    // Its OWN story: a new id, the same content, one more story in the
    // document — not a second head frame on the source's story.
    assert_ne!(story_ids[0], story);
    assert_eq!(doc.stories.len(), stories_before + 1);
    assert_eq!(story_json(&doc, &story_ids[0]), story_json(&doc, &story));
    let parent_of = |id: &str| {
        let (si, r) = locate(&doc, id);
        match r {
            FrameRef::TextFrame(i) => doc.spreads[si].spread.text_frames[i].parent_story.clone(),
            other => panic!("not a text frame: {other:?}"),
        }
    };
    assert_eq!(parent_of(&ids[0]).as_deref(), Some(story_ids[0].as_str()));
    assert_eq!(parent_of(&frame).as_deref(), Some(story.as_str()));
    // The derived index follows: each story resolves to its own frame.
    assert_eq!(
        doc.frame_for_story
            .get(&story_ids[0])
            .and_then(|f| f.self_id.clone()),
        Some(ids[0].clone())
    );

    let after = snapshot(&doc);
    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(snapshot(&doc), before, "undo removes the story it minted");
    apply(&mut doc, &undone.inverse).expect("redo");
    assert_eq!(snapshot(&doc), after);
}

fn refusal(doc: &mut Document, sources: &[NodeId]) -> String {
    let before = snapshot(doc);
    let err = apply(doc, &duplicate(sources, 1.0, 1.0)).expect_err("must be refused");
    assert_eq!(snapshot(doc), before, "a refusal writes nothing");
    // The pre-flight answers the same thing the op does.
    assert_eq!(
        duplicate_demand(doc, sources)
            .expect_err("the demand refuses too")
            .to_string(),
        err.to_string()
    );
    err.to_string()
}

#[test]
fn a_threaded_text_frame_is_refused_by_name() {
    let mut doc = fixture("text");
    let (frame, _) = plain_text_frame(&doc);
    let other = doc
        .spreads
        .iter()
        .flat_map(|p| p.spread.text_frames.iter())
        .filter_map(|f| f.self_id.clone())
        .find(|id| *id != frame)
        .expect("a second text frame");
    // Thread `frame` → `other` the way a link does.
    let (si, r) = locate(&doc, &frame);
    let FrameRef::TextFrame(i) = r else {
        panic!("text frame");
    };
    doc.spreads[si].spread.text_frames[i].next_text_frame = Some(other.clone());
    let why = refusal(&mut doc, &[NodeId::TextFrame(frame)]);
    assert!(why.contains("threaded"), "{why}");
    // …and the frame it flows INTO is just as threaded.
    let why = refusal(&mut doc, &[NodeId::TextFrame(other)]);
    assert!(why.contains("threaded"), "{why}");
}

#[test]
fn a_story_with_ids_of_its_own_is_refused_by_name() {
    for (sample, what) in [("tables", "a table"), ("anchored", "an anchored object")] {
        let mut doc = fixture(sample);
        let frame = doc
            .spreads
            .iter()
            .flat_map(|p| p.spread.text_frames.iter())
            .find_map(|f| {
                let story = doc
                    .stories
                    .iter()
                    .find(|s| Some(&s.self_id) == f.parent_story.as_ref())?;
                let hit = story.story.paragraphs.iter().any(|p| match sample {
                    "tables" => p.table.is_some(),
                    _ => !p.anchored_frames.is_empty(),
                });
                (hit && f.next_text_frame.as_deref().is_none_or(|n| n == "n"))
                    .then(|| f.self_id.clone())
                    .flatten()
            })
            .unwrap_or_else(|| panic!("{sample} carries a frame whose story holds {what}"));
        let why = refusal(&mut doc, &[NodeId::TextFrame(frame)]);
        assert!(
            why.contains(what) || why.contains("threaded"),
            "{sample}: {why}"
        );
    }
}

// ── refusals ────────────────────────────────────────────────────────

#[test]
fn what_is_not_a_page_item_on_a_spread_is_refused() {
    let mut doc = fixture("geometry-groups");
    let spread = doc.spreads[0].spread.self_id.clone().expect("spread id");
    let why = refusal(&mut doc, &[NodeId::Spread(spread)]);
    assert!(why.contains("only a page item"), "{why}");

    let story = doc.stories[0].self_id.clone();
    let why = refusal(&mut doc, &[NodeId::Story(story)]);
    assert!(why.contains("only a page item"), "{why}");

    let why = refusal(&mut doc, &[NodeId::Polygon("no-such-path".to_string())]);
    assert!(why.contains("node not found"), "{why}");

    let why = refusal(&mut doc, &[]);
    assert!(why.contains("no elements"), "{why}");
}

#[test]
fn an_anchored_object_is_refused_by_name() {
    let mut doc = fixture("anchored");
    let anchored = doc
        .stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .flat_map(|p| p.anchored_frames.iter())
        .find_map(|a| a.self_id.clone())
        .expect("the fixture anchors a frame in a story");
    let why = refusal(&mut doc, &[NodeId::Rectangle(anchored)]);
    assert!(why.contains("anchored in a story"), "{why}");
}

#[test]
fn a_mask_and_a_masked_item_are_refused_by_name() {
    let mut doc = fixture("geometry-groups");
    let target = add_rect(&mut doc, "c64target", [100.0, 100.0, 300.0, 300.0]);
    let mask = add_rect(&mut doc, "c64mask", [120.0, 120.0, 280.0, 280.0]);
    apply(
        &mut doc,
        &Operation::ApplyOpacityMask {
            target: target.clone(),
            mask: mask.clone(),
            mask_type: OpacityMaskMode::default(),
            invert: false,
        },
    )
    .expect("apply mask");
    let why = refusal(&mut doc, &[mask]);
    assert!(why.contains("serving as an opacity mask"), "{why}");
    let why = refusal(&mut doc, &[target]);
    assert!(why.contains("carries an opacity mask"), "{why}");
}

/// A group and one of its members in ONE op: the group's clone already
/// carries a copy of the member, and the member's own clone would land
/// inside the source group — two answers to one request. Refused,
/// naming the inner element, in either order.
#[test]
fn an_element_named_with_its_own_group_is_refused() {
    let mut doc = fixture("geometry-groups");
    let a = add_rect(&mut doc, "c64a", [100.0, 100.0, 300.0, 300.0]);
    let b = add_rect(&mut doc, "c64b", [150.0, 150.0, 350.0, 350.0]);
    let grouped = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![a.clone(), b],
                parent: None,
                item_transform: None,
                opacity: None,
                blend_mode: None,
            },
        },
    )
    .expect("group");
    let Operation::CreateGroup { spec } = grouped.op else {
        panic!("group echo");
    };
    let group = NodeId::Group(spec.self_id.expect("minted group id"));
    for sources in [
        vec![group.clone(), a.clone()],
        vec![a.clone(), group.clone()],
    ] {
        let why = refusal(&mut doc, &sources);
        assert!(why.contains("inside another element"), "{why}");
        assert!(
            why.contains("c64a"),
            "the INNER element is the one named: {why}"
        );
    }
    // Each alone is fine, and so is the same element twice.
    apply(&mut doc, &duplicate(&[group], 1.0, 1.0)).expect("the group alone");
    apply(&mut doc, &duplicate(&[a.clone(), a], 1.0, 1.0)).expect("one element, twice");
}

/// One bad source refuses the whole op: the good one is not cloned.
#[test]
fn a_refusal_is_all_or_nothing() {
    let mut doc = fixture("geometry-groups");
    let good = add_path(&mut doc, "c64good");
    let why = refusal(
        &mut doc,
        &[good, NodeId::Polygon("no-such-path".to_string())],
    );
    assert!(why.contains("node not found"), "{why}");
}

// ── ids ─────────────────────────────────────────────────────────────

#[test]
fn supplied_ids_are_used_and_checked() {
    let mut doc = fixture("geometry-groups");
    let node = add_path(&mut doc, "c64path");
    let with = |ids: &[&str]| Operation::DuplicateNodes {
        sources: vec![node.clone()],
        dx: 1.0,
        dy: 1.0,
        ids: ids.iter().map(|s| s.to_string()).collect(),
        story_ids: Vec::new(),
    };
    let before = snapshot(&doc);
    // Too many, and an id that is already in the document.
    let err = apply(&mut doc, &with(&["a", "b"])).expect_err("count mismatch");
    assert!(err.to_string().contains("needs 1 item id"), "{err}");
    let err = apply(&mut doc, &with(&["c64path"])).expect_err("collision");
    assert!(
        matches!(err, OperationError::DuplicateNodeId { .. }),
        "{err:?}"
    );
    assert_eq!(snapshot(&doc), before);

    let applied = apply(&mut doc, &with(&["c64clone"])).expect("supplied id");
    assert_eq!(minted(&applied).0, vec!["c64clone".to_string()]);
    assert_same_node_moved(&doc, "c64path", "c64clone", 1.0, 1.0);
}

/// Two sources in one op: each clone lands above ITS source, the roots
/// come back in source order, and one inverse removes both.
#[test]
fn several_sources_each_land_above_their_own_source() {
    let mut doc = fixture("geometry-groups");
    let a = add_rect(&mut doc, "c64a", [100.0, 100.0, 200.0, 200.0]);
    let b = add_rect(&mut doc, "c64b", [300.0, 100.0, 400.0, 200.0]);
    let before = snapshot(&doc);
    let applied = apply(&mut doc, &duplicate(&[a, b], 8.0, 8.0)).expect("duplicate two");
    let (ids, _) = minted(&applied);
    assert_eq!(
        duplicate_roots(&doc, &applied.op),
        vec![
            NodeId::Rectangle(ids[0].clone()),
            NodeId::Rectangle(ids[1].clone())
        ]
    );
    let spread = &doc.spreads[0].spread;
    let order: Vec<String> = spread
        .frames_in_order
        .iter()
        .filter_map(|r| match r {
            FrameRef::Rectangle(i) => spread.rectangles[*i].self_id.clone(),
            _ => None,
        })
        .filter(|id| id.starts_with("c64") || ids.contains(id))
        .collect();
    assert_eq!(
        order,
        vec![
            "c64a".to_string(),
            ids[0].clone(),
            "c64b".to_string(),
            ids[1].clone()
        ]
    );
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(snapshot(&doc), before, "one undo step removes both");
}
