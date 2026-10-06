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

//! C-75 — undo of a delete restores the node that was deleted.
//!
//! `RemoveNode` captured its node as a `NodeSpec`: a hand-listed set of
//! fields (id, bounds, fill, stroke colour and weight, transform, and
//! the path tables for the path kinds). Its inverse re-inserted that, so
//! everything outside the list came back at its default — opacity 40 →
//! none, nonprinting → false, corner radius 12 → none, a placed image
//! gone. An untouched fresh frame round-tripped with no difference at
//! all, which is why every delete sandwich was green.
//!
//! The node is now captured WHOLE, so this file compares whole nodes:
//! the document before the delete and after the undo, as one value. A
//! field the model gains later is covered without touching this file.

use std::path::PathBuf;

use paged_model::FrameRef;
use paged_mutate::operation::{NodeSpec, PathAnchorSpec};
use paged_mutate::{apply, NodeId, Operation, PropertyPath, Value};
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
    let stories: Vec<Json> = doc
        .stories
        .iter()
        .map(|s| serde_json::json!({ "id": s.self_id, "story": s.story }))
        .collect();
    serde_json::json!({ "spreads": spreads, "stories": stories })
}

fn spread_id(doc: &Document) -> String {
    doc.spreads[0].spread.self_id.clone().expect("spread id")
}

fn corner(x: f32, y: f32) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    }
}

/// Insert one bare item of each leaf kind, on top; returns the nodes.
fn one_of_each(doc: &mut Document) -> Vec<NodeId> {
    let spread = NodeId::Spread(spread_id(doc));
    let specs = vec![
        (
            doc.spreads[0].spread.rectangles.len(),
            NodeSpec::Rectangle {
                self_id: "c75rect".to_string(),
                bounds: [100.0, 100.0, 180.0, 220.0],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(2.0),
                item_transform: Some([1.0, 0.0, 0.0, 1.0, 12.0, 7.0]),
            },
        ),
        (
            doc.spreads[0].spread.ovals.len(),
            NodeSpec::Oval {
                self_id: "c75oval".to_string(),
                bounds: [100.0, 260.0, 180.0, 380.0],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(2.0),
                item_transform: None,
            },
        ),
        (
            doc.spreads[0].spread.polygons.len(),
            NodeSpec::Polygon {
                self_id: "c75poly".to_string(),
                bounds: [220.0, 100.0, 300.0, 220.0],
                anchors: vec![
                    corner(100.0, 220.0),
                    corner(220.0, 220.0),
                    corner(220.0, 300.0),
                    corner(100.0, 300.0),
                ],
                subpath_starts: vec![0],
                subpath_open: vec![false],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(2.0),
                item_transform: None,
            },
        ),
        (
            doc.spreads[0].spread.graphic_lines.len(),
            NodeSpec::GraphicLine {
                self_id: "c75line".to_string(),
                bounds: [220.0, 260.0, 300.0, 380.0],
                anchors: vec![corner(260.0, 220.0), corner(380.0, 300.0)],
                subpath_starts: vec![],
                subpath_open: vec![],
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(2.0),
                item_transform: None,
            },
        ),
        (
            doc.spreads[0].spread.text_frames.len(),
            NodeSpec::TextFrame {
                self_id: "c75text".to_string(),
                bounds: [340.0, 100.0, 420.0, 220.0],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(2.0),
                item_transform: None,
                parent_story: Some("Story/c75".to_string()),
            },
        ),
    ];
    let mut nodes = Vec::new();
    for (position, node) in specs {
        nodes.push(node.node_id());
        let parent = spread.clone();
        apply(
            doc,
            &Operation::InsertNode {
                parent,
                position,
                z_slot: None,
                node,
            },
        )
        .expect("insert");
    }
    nodes
}

/// Every property this kind takes, set to something that is not its
/// default. A path the kind does not support is skipped — what matters
/// is that whatever DID land survives the round trip, and the caller
/// asserts that plenty landed.
fn dress(doc: &mut Document, node: &NodeId) -> usize {
    use PropertyPath as P;
    let text = |s: &str| Value::Text(s.to_string());
    let len = |v: f32| Value::Length(Some(v));
    let writes: Vec<(PropertyPath, Value)> = vec![
        (P::FrameOpacity, len(40.0)),
        (P::FrameBlendMode, text("Multiply")),
        (P::FrameFillTint, len(55.0)),
        (P::FrameNonprinting, Value::Bool(true)),
        (P::FrameCornerRadiusTopLeft, len(12.0)),
        (P::FrameCornerOptionTopLeft, text("RoundedCorner")),
        (P::FrameCornerRadiusBottomRight, len(7.0)),
        (P::FrameStrokeType, text("StrokeStyle/$ID/Dashed")),
        (P::FrameStrokeJoin, text("RoundEndJoin")),
        (P::FrameStrokeMiterLimit, len(6.0)),
        (P::FrameStrokeEndCap, text("RoundEndCap")),
        (P::FrameStrokeAlignment, text("InsideAlignment")),
        (
            P::FrameStrokeGapColor,
            Value::ColorRef(Some("Color/Black".to_string())),
        ),
        (P::FrameStrokeGapTint, len(30.0)),
        (P::FrameStrokeDashArray, Value::Lengths(vec![6.0, 3.0])),
        (P::FrameOverprintFill, Value::Bool(true)),
        (P::FrameOverprintStroke, Value::Bool(true)),
        (P::FrameDropShadow, Value::Bool(true)),
        (P::FrameDropShadowSize, len(9.0)),
        (P::FrameInnerShadowEnabled, Value::Bool(true)),
        (P::FrameOuterGlowEnabled, Value::Bool(true)),
        (P::FrameOuterGlowSize, len(13.5)),
        (P::FrameBevelEnabled, Value::Bool(true)),
        (P::FrameFeatherEnabled, Value::Bool(true)),
        (P::FrameGradientFillAngle, len(33.0)),
        (P::FrameGradientFillLength, len(120.0)),
        (P::FrameStrokeStartArrowhead, text("TriangleArrowHead")),
        (P::FrameStrokeEndArrowhead, text("CircleSolidArrowHead")),
        (P::TextFrameColumnCount, len(3.0)),
        (P::TextFrameColumnGutter, len(14.0)),
        (
            P::PluginMetadata,
            Value::PluginMetadata {
                key: "x-paged:c75".to_string(),
                value: Some(r#"{"v":1,"data":{"k":"v"}}"#.to_string()),
                caller: None,
                prev: None,
            },
        ),
    ];
    let mut landed = 0;
    for (path, value) in writes {
        if apply(
            doc,
            &Operation::SetProperty {
                node: node.clone(),
                path,
                value,
            },
        )
        .is_ok()
        {
            landed += 1;
        }
    }
    landed
}

fn remove(node: &NodeId) -> Operation {
    Operation::RemoveNode { node: node.clone() }
}

#[test]
fn deleting_and_undoing_restores_the_whole_node_for_every_kind() {
    let mut doc = fixture();
    let nodes = one_of_each(&mut doc);
    for node in &nodes {
        let landed = dress(&mut doc, node);
        assert!(
            landed >= 6,
            "{node:?}: only {landed} writes landed — the test is not dressing the node"
        );
    }
    // A placed image on the rectangle: the loss the editor could detect.
    apply(
        &mut doc,
        &Operation::ReplaceImageBytes {
            frame: nodes[0].clone(),
            bytes: Some(vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4, 5]),
            prior_has_image_element: None,
        },
    )
    .expect("image bytes");

    for node in &nodes {
        let before = snapshot(&doc);
        let applied =
            apply(&mut doc, &remove(node)).unwrap_or_else(|e| panic!("remove {node:?}: {e:?}"));
        assert_ne!(snapshot(&doc), before, "{node:?}: the node is gone");
        let undone = apply(&mut doc, &applied.inverse)
            .unwrap_or_else(|e| panic!("undo of removing {node:?}: {e:?}"));
        let after = snapshot(&doc);
        if after != before {
            // Name the fields, so a failure says WHAT was lost.
            let id = node.self_id();
            let pick = |snap: &Json| -> Json {
                let spread = &snap["spreads"][0]["spread"];
                for vec in [
                    "text_frames",
                    "rectangles",
                    "ovals",
                    "graphic_lines",
                    "polygons",
                ] {
                    if let Some(found) = spread[vec]
                        .as_array()
                        .and_then(|a| a.iter().find(|n| n["self_id"] == id))
                    {
                        return found.clone();
                    }
                }
                Json::Null
            };
            let (b, a) = (pick(&before), pick(&after));
            let lost: Vec<String> = b
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter(|(k, v)| a.get(k.as_str()) != Some(*v))
                        .map(|(k, v)| {
                            format!("{k}: {v} → {}", a.get(k.as_str()).unwrap_or(&Json::Null))
                        })
                        .collect()
                })
                .unwrap_or_default();
            panic!(
                "{node:?}: undo of the delete did not restore the node. Fields that differ:\n  {}\n\
                 (an empty list means the difference is outside the node: z-order or a side map)",
                lost.join("\n  ")
            );
        }
        // Redo removes it again, and a second undo is as exact.
        apply(&mut doc, &undone.inverse).expect("redo");
        apply(&mut doc, &applied.inverse).expect("undo again");
        assert_eq!(
            snapshot(&doc),
            before,
            "{node:?}: a second undo is exact too"
        );
    }
}

/// The side maps keyed by the item's id go with it. They used to stay
/// behind — and ids are minted as "highest in the document + 1", so the
/// next item created after deleting the newest one took the SAME id and
/// with it the deleted item's plugin metadata.
#[test]
fn a_new_item_does_not_inherit_a_deleted_items_metadata() {
    let mut doc = fixture();
    let nodes = one_of_each(&mut doc);
    let rect = &nodes[0];
    dress(&mut doc, rect);
    assert!(doc.spreads[0].spread.labels.contains_key("c75rect"));
    apply(&mut doc, &remove(rect)).expect("remove");
    assert!(
        !doc.spreads[0].spread.labels.contains_key("c75rect"),
        "the deleted item's labels leave with it"
    );
    // The same id again, as a recycled mint would produce.
    let position = doc.spreads[0].spread.rectangles.len();
    let parent = NodeId::Spread(spread_id(&doc));
    apply(
        &mut doc,
        &Operation::InsertNode {
            parent,
            position,
            z_slot: None,
            node: NodeSpec::Rectangle {
                self_id: "c75rect".to_string(),
                bounds: [0.0, 0.0, 10.0, 10.0],
                fill_color: None,
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
        },
    )
    .expect("a new item under the recycled id");
    assert!(
        !doc.spreads[0].spread.labels.contains_key("c75rect"),
        "a fresh item carries no metadata of its predecessor"
    );
}

/// A text frame's story comes back with it (it always did), and so does
/// the frame's own formatting (it did not).
#[test]
fn a_text_frame_comes_back_with_its_story_and_its_formatting() {
    let mut doc = fixture();
    let nodes = one_of_each(&mut doc);
    let frame = nodes
        .iter()
        .find(|n| matches!(n, NodeId::TextFrame(_)))
        .expect("text frame");
    dress(&mut doc, frame);
    let (si, idx) = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, p)| {
            p.spread
                .text_frames
                .iter()
                .position(|f| f.self_id.as_deref() == Some("c75text"))
                .map(|i| (si, i))
        })
        .expect("the frame");
    let before = doc.spreads[si].spread.text_frames[idx].clone();
    assert_eq!(before.column_count, Some(3));
    let z_before = doc.spreads[si]
        .spread
        .frames_in_order
        .iter()
        .position(|r| *r == FrameRef::TextFrame(idx));

    let applied = apply(&mut doc, &remove(frame)).expect("remove");
    apply(&mut doc, &applied.inverse).expect("undo");
    let after = &doc.spreads[si].spread.text_frames[idx];
    assert_eq!(after.parent_story, before.parent_story, "the story");
    assert_eq!(after.column_count, Some(3), "the columns");
    assert_eq!(after.opacity, Some(40.0), "the opacity");
    assert_eq!(
        doc.spreads[si]
            .spread
            .frames_in_order
            .iter()
            .position(|r| *r == FrameRef::TextFrame(idx)),
        z_before,
        "the z slot"
    );
}
