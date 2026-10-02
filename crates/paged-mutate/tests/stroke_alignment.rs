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

//! C-24 — `FrameStrokeAlignment` is writable on every kind that DRAWS it.
//!
//! The model carries `stroke_alignment` on TextFrame, Rectangle, Oval and
//! Polygon, the importer fills all four, and the renderer offsets all
//! four outlines by ±weight/2 (W1.5). Only the Rectangle could be
//! written; the element descriptor even READ the Polygon's value and
//! then refused the write.
//!
//! Pinned here, per kind: the write lands, undo restores, redo
//! reproduces; the page paints differently; and for the Polygon the
//! stroked outline moves by exactly half the weight on each side, in the
//! direction the alignment names. A `GraphicLine` has no such field — an
//! open stroke has no inside — and keeps rejecting.

use std::path::PathBuf;

use paged_compose::{DisplayCommand, PathSegment};
use paged_mutate::operation::{NodeSpec, PathAnchorSpec};
use paged_mutate::{apply, NodeId, Operation, OperationError, PropertyPath, Value};
use paged_renderer::pipeline::{build_document, PipelineOptions};
use paged_scene::Document;

const WEIGHT: f32 = 8.0;

fn fixture() -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml");
    let bytes = std::fs::read(path).expect("read geometry-groups fixture");
    idml_import::import_idml_doc(&bytes).expect("open")
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

fn align(node: &NodeId, token: &str) -> Operation {
    set(
        node,
        PropertyPath::FrameStrokeAlignment,
        Value::Text(token.to_string()),
    )
}

/// Give `node` a stroke the renderer will draw.
fn stroke(doc: &mut Document, node: &NodeId) {
    apply(
        doc,
        &set(
            node,
            PropertyPath::FrameStrokeColor,
            Value::ColorRef(Some("Color/Black".to_string())),
        ),
    )
    .expect("stroke colour");
    apply(
        doc,
        &set(
            node,
            PropertyPath::FrameStrokeWeight,
            Value::Length(Some(WEIGHT)),
        ),
    )
    .expect("stroke weight");
}

/// Append a leaf item to the first spread, on top.
fn insert(doc: &mut Document, node: NodeSpec) -> NodeId {
    let spread = &doc.spreads[0].spread;
    let position = match &node {
        NodeSpec::Oval { .. } => spread.ovals.len(),
        NodeSpec::GraphicLine { .. } => spread.graphic_lines.len(),
        NodeSpec::Polygon { .. } => spread.polygons.len(),
        other => panic!("this helper appends ovals, lines and polygons, not {other:?}"),
    };
    let id = node.node_id();
    let parent = NodeId::Spread(spread.self_id.clone().expect("the first spread has an id"));
    apply(
        doc,
        &Operation::InsertNode {
            parent,
            position,
            z_slot: None,
            node,
        },
    )
    .expect("insert node");
    id
}

fn corner(x: f32, y: f32) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    }
}

/// A closed 200 × 100 quad — what the pen mints.
fn quad(doc: &mut Document) -> NodeId {
    insert(
        doc,
        NodeSpec::Polygon {
            self_id: "c24poly".to_string(),
            bounds: [100.0, 100.0, 200.0, 300.0],
            anchors: vec![
                corner(100.0, 100.0),
                corner(300.0, 100.0),
                corner(300.0, 200.0),
                corner(100.0, 200.0),
            ],
            subpath_starts: vec![0],
            subpath_open: vec![false],
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            item_transform: None,
        },
    )
}

fn oval(doc: &mut Document) -> NodeId {
    insert(
        doc,
        NodeSpec::Oval {
            self_id: "c24oval".to_string(),
            bounds: [300.0, 100.0, 400.0, 300.0],
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            item_transform: None,
        },
    )
}

fn first_text_frame(doc: &Document) -> NodeId {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find_map(|f| f.self_id.clone())
        .map(NodeId::TextFrame)
        .expect("fixture has a text frame")
}

fn first_rectangle(doc: &Document) -> NodeId {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find_map(|r| r.self_id.clone())
        .map(NodeId::Rectangle)
        .expect("fixture has a rectangle")
}

fn alignment_of(doc: &Document, node: &NodeId) -> Option<String> {
    let id = node.self_id();
    for parsed in &doc.spreads {
        let s = &parsed.spread;
        let found = match node {
            NodeId::TextFrame(_) => s
                .text_frames
                .iter()
                .find(|f| f.self_id.as_deref() == Some(id))
                .map(|f| f.stroke_alignment.clone()),
            NodeId::Rectangle(_) => s
                .rectangles
                .iter()
                .find(|f| f.self_id.as_deref() == Some(id))
                .map(|f| f.stroke_alignment.clone()),
            NodeId::Oval(_) => s
                .ovals
                .iter()
                .find(|f| f.self_id.as_deref() == Some(id))
                .map(|f| f.stroke_alignment.clone()),
            NodeId::Polygon(_) => s
                .polygons
                .iter()
                .find(|f| f.self_id.as_deref() == Some(id))
                .map(|f| f.stroke_alignment.clone()),
            other => panic!("{other:?} carries no stroke alignment"),
        };
        if let Some(v) = found {
            return v;
        }
    }
    panic!("{node:?} not found");
}

fn commands(doc: &Document) -> Vec<String> {
    build_document(doc, &PipelineOptions::default())
        .expect("build")
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter().map(|c| format!("{c:?}")))
        .collect()
}

#[test]
fn every_kind_that_draws_the_alignment_takes_the_write_and_undoes_it() {
    let mut doc = fixture();
    let nodes = [
        ("Rectangle", first_rectangle(&doc)),
        ("Polygon", quad(&mut doc)),
        ("Oval", oval(&mut doc)),
        ("TextFrame", first_text_frame(&doc)),
    ];
    for (kind, node) in nodes {
        stroke(&mut doc, &node);
        let before = alignment_of(&doc, &node);
        let centred = commands(&doc);

        let applied = apply(&mut doc, &align(&node, "InsideAlignment"))
            .unwrap_or_else(|e| panic!("{kind} must accept FrameStrokeAlignment (C-24): {e:?}"));
        assert_eq!(
            alignment_of(&doc, &node).as_deref(),
            Some("InsideAlignment"),
            "{kind}: the write lands on the item's own field"
        );
        assert_eq!(applied.invalidation.frame_style, vec![node.clone()]);
        // Only a text frame reflows: the stroke's share insets its text.
        assert_eq!(
            applied.invalidation.text_reflow.len(),
            usize::from(matches!(node, NodeId::TextFrame(_))),
            "{kind}: reflow hint"
        );
        assert_ne!(
            commands(&doc),
            centred,
            "{kind}: an inside-aligned stroke paints differently"
        );

        let undone = apply(&mut doc, &applied.inverse).expect("undo");
        assert_eq!(alignment_of(&doc, &node), before, "{kind}: undo restores");
        assert_eq!(commands(&doc), centred, "{kind}: undo repaints as before");

        apply(&mut doc, &undone.inverse).expect("redo");
        assert_eq!(
            alignment_of(&doc, &node).as_deref(),
            Some("InsideAlignment"),
            "{kind}: redo reproduces"
        );
        // The empty string clears the override, as on the Rectangle.
        apply(&mut doc, &align(&node, "")).expect("clear");
        assert_eq!(alignment_of(&doc, &node), None, "{kind}: \"\" clears");
    }
}

/// `(min x, min y, max x, max y)` of the polygon's stroked outline — the
/// one `StrokePath` on the page whose path is a four-corner contour of
/// about the quad's size.
fn stroked_outline(doc: &Document) -> (f32, f32, f32, f32) {
    let built = build_document(doc, &PipelineOptions::default()).expect("build");
    let mut found = Vec::new();
    for page in &built.pages {
        for c in &page.list.commands {
            let DisplayCommand::StrokePath { path_id, .. } = c else {
                continue;
            };
            let Some(path) = page.list.paths.get(*path_id) else {
                continue;
            };
            let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            let mut grow = |x: f32, y: f32| {
                b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
            };
            for seg in &path.segments {
                match *seg {
                    PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => grow(x, y),
                    PathSegment::CubicTo { x, y, .. } => grow(x, y),
                    _ => {}
                }
            }
            let (w, h) = (b.2 - b.0, b.3 - b.1);
            if (w - 200.0).abs() <= WEIGHT + 0.5 && (h - 100.0).abs() <= WEIGHT + 0.5 {
                found.push(b);
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "exactly one stroked quad outline: {found:?}"
    );
    found[0]
}

/// The geometry, not just "something changed": inside pulls every edge
/// in by half the weight, outside pushes it out by the same.
#[test]
fn a_polygon_outline_moves_by_half_the_weight() {
    let mut doc = fixture();
    let node = quad(&mut doc);
    stroke(&mut doc, &node);
    let half = WEIGHT / 2.0;
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;

    let c = stroked_outline(&doc);
    assert!(
        near(c.2 - c.0, 200.0) && near(c.3 - c.1, 100.0),
        "a centred stroke rides the path itself: {c:?}"
    );

    apply(&mut doc, &align(&node, "InsideAlignment")).expect("inside");
    let i = stroked_outline(&doc);
    assert!(
        near(i.0, c.0 + half) && near(i.1, c.1 + half),
        "inside moves the top-left corner in by {half}: {i:?} vs {c:?}"
    );
    assert!(
        near(i.2, c.2 - half) && near(i.3, c.3 - half),
        "inside moves the bottom-right corner in by {half}: {i:?} vs {c:?}"
    );

    apply(&mut doc, &align(&node, "OutsideAlignment")).expect("outside");
    let o = stroked_outline(&doc);
    assert!(
        near(o.0, c.0 - half) && near(o.1, c.1 - half),
        "outside moves the top-left corner out by {half}: {o:?} vs {c:?}"
    );
    assert!(
        near(o.2, c.2 + half) && near(o.3, c.3 + half),
        "outside moves the bottom-right corner out by {half}: {o:?} vs {c:?}"
    );

    apply(&mut doc, &align(&node, "CenterAlignment")).expect("centre");
    assert_eq!(stroked_outline(&doc), c, "centre is the path again");
}

#[test]
fn a_graphic_line_has_no_inside_and_rejects() {
    let mut doc = fixture();
    let node = insert(
        &mut doc,
        NodeSpec::GraphicLine {
            self_id: "c24line".to_string(),
            bounds: [40.0, 40.0, 140.0, 140.0],
            anchors: vec![corner(40.0, 40.0), corner(140.0, 140.0)],
            subpath_starts: vec![],
            subpath_open: vec![],
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight: Some(1.0),
            item_transform: None,
        },
    );
    let err = apply(&mut doc, &align(&node, "InsideAlignment"))
        .expect_err("an open stroke has no inside to align to");
    assert!(
        matches!(err, OperationError::UnsupportedProperty { .. }),
        "GraphicLine must answer UnsupportedProperty, got {err:?}"
    );
}
