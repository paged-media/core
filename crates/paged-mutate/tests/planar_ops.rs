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

//! B-22 — the region-level Pathfinder verbs (`Divide` / `Trim` /
//! `Merge` / `Crop` / `Outline` / `MinusBack`) and Shape Builder's
//! `PathfinderFaces` through the apply layer: what each verb produces,
//! who owns the results, and — for every one of them — that a SINGLE
//! inverse restores the originals exactly.

use std::path::PathBuf;

use paged_mutate::operation::PathAnchorSpec;
use paged_mutate::{
    apply, FaceSelectMode, NodeId, Operation, PathfinderRegionVerb, PropertyPath, Value,
};
use paged_scene::Document;

fn fixture_doc() -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml");
    let bytes = std::fs::read(path).expect("read geometry fixture");
    idml_import::import_idml_doc(&bytes).expect("open")
}

fn rect_specs(left: f32, top: f32, right: f32, bottom: f32) -> Vec<PathAnchorSpec> {
    [(left, top), (right, top), (right, bottom), (left, bottom)]
        .into_iter()
        .map(|(x, y)| PathAnchorSpec {
            anchor: [x, y],
            left: [x, y],
            right: [x, y],
        })
        .collect()
}

/// Give a rectangle an explicit, known path (the fixture's rectangles
/// are primitive `GeometricBounds` frames, all at the same box).
fn set_rect_path(doc: &mut Document, node: &NodeId, l: f32, t: f32, r: f32, b: f32) {
    apply(
        doc,
        &Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::ClosePath,
            value: Value::ClosePath {
                subpath: None,
                prev_anchors: Some(rect_specs(l, t, r, b)),
                prev_subpath_starts: Some(vec![0]),
                prev_subpath_open: Some(vec![false]),
            },
        },
    )
    .expect("seed path");
}

/// Two overlapping squares on spread 0: `A` = [0,20]² on top, `B` =
/// [10,30]² beneath it.
fn two_squares() -> (Document, Vec<NodeId>) {
    let mut doc = fixture_doc();
    let ids: Vec<String> = doc.spreads[0]
        .spread
        .rectangles
        .iter()
        .filter_map(|r| r.self_id.clone())
        .collect();
    assert_eq!(ids.len(), 2, "spread 0 carries two rectangles");
    let a = NodeId::Rectangle(ids[0].clone());
    let b = NodeId::Rectangle(ids[1].clone());
    set_rect_path(&mut doc, &a, 0.0, 0.0, 20.0, 20.0);
    set_rect_path(&mut doc, &b, 10.0, 10.0, 30.0, 30.0);
    (doc, vec![a, b])
}

// ---------------------------------------------------------------------------
// Snapshot helpers
// ---------------------------------------------------------------------------

type AnchorKey = ((f32, f32), (f32, f32), (f32, f32));

#[derive(Debug, PartialEq)]
struct Shape {
    id: String,
    anchors: Vec<AnchorKey>,
    starts: Vec<usize>,
    open: Vec<bool>,
    fill: Option<String>,
    stroke: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    rectangles: Vec<Shape>,
    polygons: Vec<Shape>,
    lines: Vec<Shape>,
}

fn snapshot(doc: &Document) -> Snapshot {
    let s = &doc.spreads[0].spread;
    let keys = |anchors: &[paged_model::PathAnchor]| -> Vec<AnchorKey> {
        anchors
            .iter()
            .map(|a| (a.anchor, a.left, a.right))
            .collect()
    };
    Snapshot {
        rectangles: s
            .rectangles
            .iter()
            .map(|r| Shape {
                id: r.self_id.clone().unwrap_or_default(),
                anchors: keys(&r.anchors),
                starts: r.subpath_starts.clone(),
                open: r.subpath_open.clone(),
                fill: r.fill_color.clone(),
                stroke: r.stroke_color.clone(),
            })
            .collect(),
        polygons: s
            .polygons
            .iter()
            .map(|p| Shape {
                id: p.self_id.clone().unwrap_or_default(),
                anchors: keys(&p.anchors),
                starts: p.subpath_starts.clone(),
                open: p.subpath_open.clone(),
                fill: p.fill_color.clone(),
                stroke: p.stroke_color.clone(),
            })
            .collect(),
        lines: s
            .graphic_lines
            .iter()
            .map(|l| Shape {
                id: l.self_id.clone().unwrap_or_default(),
                anchors: keys(&l.anchors),
                starts: l.subpath_starts.clone(),
                open: l.subpath_open.clone(),
                fill: None,
                stroke: l.stroke_color.clone(),
            })
            .collect(),
    }
}

/// Shoelace area of a corner-anchor contour set (every fixture path
/// here is a straight-edged polygon). Holes cancel outers, so the sum's
/// magnitude is the filled area — the check that catches a "union" that
/// secretly double-covers or leaves an internal edge behind.
fn polygon_area(shape: &Shape) -> f32 {
    let starts: Vec<usize> = if shape.starts.is_empty() {
        vec![0]
    } else {
        shape.starts.clone()
    };
    let mut total = 0.0;
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(shape.anchors.len());
        let ring = &shape.anchors[*start..end];
        let mut acc = 0.0;
        for k in 0..ring.len() {
            let (a, _, _) = ring[k];
            let (b, _, _) = ring[(k + 1) % ring.len()];
            acc += a.0 * b.1 - b.0 * a.1;
        }
        total += acc / 2.0;
    }
    total.abs()
}

fn anchor_bbox(anchors: &[AnchorKey]) -> (f32, f32, f32, f32) {
    let mut out = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    for (a, _, _) in anchors {
        out.0 = out.0.min(a.0);
        out.1 = out.1.min(a.1);
        out.2 = out.2.max(a.0);
        out.3 = out.3.max(a.1);
    }
    out
}

/// Run `op`, assert the document changed, then assert ONE inverse puts
/// it back exactly — the faithful-inverse contract every verb owes.
fn assert_one_undo_restores(doc: &mut Document, op: Operation) -> Snapshot {
    let before = snapshot(doc);
    let applied = apply(doc, &op).expect("forward apply");
    let after = snapshot(doc);
    assert_ne!(before, after, "the verb must change the document");
    apply(doc, &applied.inverse).expect("inverse apply");
    assert_eq!(snapshot(doc), before, "one undo restores the originals");
    after
}

// ---------------------------------------------------------------------------
// Divide
// ---------------------------------------------------------------------------

#[test]
fn divide_two_overlapping_squares_makes_one_element_per_face() {
    let (mut doc, elements) = two_squares();
    let cyan = doc.spreads[0].spread.rectangles[0].fill_color.clone();
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements,
            verb: PathfinderRegionVerb::Divide,
        },
    );
    // Three faces: A-only (L), A∩B (square), B-only (L). The two
    // inputs carry one each; the surplus face becomes a new Polygon.
    assert_eq!(after.rectangles.len(), 2);
    assert_eq!(after.polygons.len(), 1);
    // The overlap is owned by the TOPMOST input covering it — A — so it
    // inherits A's fill, not B's.
    assert_eq!(after.polygons[0].fill, cyan);
    assert_eq!(
        anchor_bbox(&after.polygons[0].anchors),
        (10.0, 10.0, 20.0, 20.0),
        "the surplus face is the overlap square"
    );
    // A keeps the L-shape it exclusively covers (6 corners), B likewise
    // — 400 − 100 = 300 pt² each.
    assert_eq!(after.rectangles[0].anchors.len(), 6);
    assert_eq!(after.rectangles[1].anchors.len(), 6);
    assert!((polygon_area(&after.rectangles[0]) - 300.0).abs() < 0.01);
    assert!((polygon_area(&after.rectangles[1]) - 300.0).abs() < 0.01);
    assert!((polygon_area(&after.polygons[0]) - 100.0).abs() < 0.01);
}

#[test]
fn divide_of_disjoint_shapes_leaves_each_element_alone() {
    let mut doc = fixture_doc();
    let ids: Vec<String> = doc.spreads[0]
        .spread
        .rectangles
        .iter()
        .filter_map(|r| r.self_id.clone())
        .collect();
    let a = NodeId::Rectangle(ids[0].clone());
    let b = NodeId::Rectangle(ids[1].clone());
    set_rect_path(&mut doc, &a, 0.0, 0.0, 10.0, 10.0);
    set_rect_path(&mut doc, &b, 40.0, 40.0, 50.0, 50.0);
    // Disjoint inputs are a no-op by construction: each element is the
    // sole face of its own signature, so it is rewritten with the path
    // it already had. Assert exactly that — including that the inverse
    // is still well-formed.
    let before = snapshot(&doc);
    let applied = apply(
        &mut doc,
        &Operation::PathfinderRegion {
            elements: vec![a, b],
            verb: PathfinderRegionVerb::Divide,
        },
    )
    .expect("forward apply");
    let after = snapshot(&doc);
    assert_eq!(after, before, "disjoint shapes divide into themselves");
    assert_eq!(after.rectangles.len(), 2);
    assert!(after.polygons.is_empty(), "no surplus faces");
    assert_eq!(
        anchor_bbox(&after.rectangles[0].anchors),
        (0.0, 0.0, 10.0, 10.0)
    );
    assert_eq!(
        anchor_bbox(&after.rectangles[1].anchors),
        (40.0, 40.0, 50.0, 50.0)
    );
    apply(&mut doc, &applied.inverse).expect("inverse apply");
    assert_eq!(snapshot(&doc), before);
}

// ---------------------------------------------------------------------------
// Trim / Merge / Crop / MinusBack / Outline
// ---------------------------------------------------------------------------

#[test]
fn trim_clips_each_object_to_what_nothing_above_it_covers() {
    let (mut doc, elements) = two_squares();
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements,
            verb: PathfinderRegionVerb::Trim,
        },
    );
    assert_eq!(after.rectangles.len(), 2, "Trim keeps one object per input");
    assert!(after.polygons.is_empty());
    // The top object keeps its whole area (nothing covers it). Its
    // outline gains the two T-junction anchors where the lower square's
    // edges met it — collinear points, same 400 pt² region.
    assert_eq!(
        anchor_bbox(&after.rectangles[0].anchors),
        (0.0, 0.0, 20.0, 20.0)
    );
    assert!((polygon_area(&after.rectangles[0]) - 400.0).abs() < 0.01);
    // …the lower one loses the hidden corner and becomes a 300 pt² L.
    assert_eq!(after.rectangles[1].anchors.len(), 6);
    assert!((polygon_area(&after.rectangles[1]) - 300.0).abs() < 0.01);
    // Illustrator's Trim "removes any strokes".
    assert_eq!(after.rectangles[0].stroke.as_deref(), Some("Swatch/None"));
    assert_eq!(after.rectangles[1].stroke.as_deref(), Some("Swatch/None"));
}

#[test]
fn trim_deletes_an_object_that_is_completely_hidden() {
    let mut doc = fixture_doc();
    let ids: Vec<String> = doc.spreads[0]
        .spread
        .rectangles
        .iter()
        .filter_map(|r| r.self_id.clone())
        .collect();
    let a = NodeId::Rectangle(ids[0].clone());
    let b = NodeId::Rectangle(ids[1].clone());
    set_rect_path(&mut doc, &a, 0.0, 0.0, 40.0, 40.0);
    set_rect_path(&mut doc, &b, 10.0, 10.0, 20.0, 20.0);
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements: vec![a, b],
            verb: PathfinderRegionVerb::Trim,
        },
    );
    assert_eq!(after.rectangles.len(), 1, "the hidden object is gone");
    assert_eq!(after.rectangles[0].id, ids[0]);
}

#[test]
fn merge_coalesces_objects_that_share_a_fill() {
    let (mut doc, elements) = two_squares();
    // Give both squares the same fill so Merge has something to merge.
    let fill = doc.spreads[0].spread.rectangles[0].fill_color.clone();
    doc.spreads[0].spread.rectangles[1].fill_color = fill.clone();
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements,
            verb: PathfinderRegionVerb::Merge,
        },
    );
    assert_eq!(after.rectangles.len(), 1, "same fill ⇒ one merged object");
    assert_eq!(after.rectangles[0].fill, fill);
    // The merged outline is the union of the two squares: one contour
    // spanning both, 400 + 400 − 100 = 700 pt².
    assert_eq!(
        anchor_bbox(&after.rectangles[0].anchors),
        (0.0, 0.0, 30.0, 30.0)
    );
    assert_eq!(after.rectangles[0].starts.len(), 1);
    assert!((polygon_area(&after.rectangles[0]) - 700.0).abs() < 0.01);
}

#[test]
fn merge_keeps_different_fills_apart() {
    let (mut doc, elements) = two_squares();
    // Fixture fills differ (cyan / magenta), so Merge degenerates to
    // Trim: two objects, neither coalesced.
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements,
            verb: PathfinderRegionVerb::Merge,
        },
    );
    assert_eq!(after.rectangles.len(), 2);
    assert!((polygon_area(&after.rectangles[0]) - 400.0).abs() < 0.01);
    assert!((polygon_area(&after.rectangles[1]) - 300.0).abs() < 0.01);
}

#[test]
fn crop_keeps_only_what_falls_inside_the_topmost_object() {
    let (mut doc, elements) = two_squares();
    let magenta = doc.spreads[0].spread.rectangles[1].fill_color.clone();
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements,
            verb: PathfinderRegionVerb::Crop,
        },
    );
    // The topmost object is the cookie cutter and is consumed; what
    // survives is the overlap, still coloured by the object BENEATH.
    assert_eq!(after.rectangles.len(), 1);
    assert_eq!(after.rectangles[0].fill, magenta);
    assert_eq!(
        anchor_bbox(&after.rectangles[0].anchors),
        (10.0, 10.0, 20.0, 20.0)
    );
    assert_eq!(after.rectangles[0].stroke.as_deref(), Some("Swatch/None"));
}

#[test]
fn minus_back_subtracts_everything_in_front_from_the_backmost() {
    let (mut doc, elements) = two_squares();
    let back_id = match &elements[1] {
        NodeId::Rectangle(id) => id.clone(),
        other => panic!("expected a rectangle, got {other:?}"),
    };
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements,
            verb: PathfinderRegionVerb::MinusBack,
        },
    );
    assert_eq!(after.rectangles.len(), 1, "only the backmost survives");
    assert_eq!(after.rectangles[0].id, back_id);
    // B minus A: the L-shape, six corners, 300 pt², spanning B's box.
    assert_eq!(after.rectangles[0].anchors.len(), 6);
    assert!((polygon_area(&after.rectangles[0]) - 300.0).abs() < 0.01);
    assert_eq!(
        anchor_bbox(&after.rectangles[0].anchors),
        (10.0, 10.0, 30.0, 30.0)
    );
}

#[test]
fn outline_converts_fills_to_stroked_segments() {
    let (mut doc, elements) = two_squares();
    let cyan = doc.spreads[0].spread.rectangles[0].fill_color.clone();
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderRegion {
            elements,
            verb: PathfinderRegionVerb::Outline,
        },
    );
    // Every input is consumed; each square's 4 sides are split at the
    // two crossings, giving 6 open segments per square.
    assert!(after.rectangles.is_empty());
    assert_eq!(after.lines.len(), 12);
    for line in &after.lines {
        assert_eq!(line.anchors.len(), 2, "one segment per arrangement edge");
        assert_eq!(line.open, vec![true], "segments are open paths");
    }
    // The stroke carries the SOURCE'S FILL — that is what "converts
    // fills to strokes" means.
    assert_eq!(
        after.lines.iter().filter(|l| l.stroke == cyan).count(),
        6,
        "the top square's six segments carry its fill as their stroke"
    );
}

// ---------------------------------------------------------------------------
// Shape Builder — PathfinderFaces
// ---------------------------------------------------------------------------

#[test]
fn faces_keep_unites_the_named_faces() {
    let (mut doc, elements) = two_squares();
    // "0#0" = the region only A covers; "0-1#0" = the overlap. Uniting
    // them recovers A exactly.
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderFaces {
            elements,
            faces: vec!["0#0".to_string(), "0-1#0".to_string()],
            mode: FaceSelectMode::Keep,
        },
    );
    assert_eq!(after.rectangles.len(), 1);
    assert!(after.polygons.is_empty());
    assert_eq!(
        anchor_bbox(&after.rectangles[0].anchors),
        (0.0, 0.0, 20.0, 20.0)
    );
    assert_eq!(
        after.rectangles[0].starts.len(),
        1,
        "adjacent faces unite into ONE contour"
    );
    assert!(
        (polygon_area(&after.rectangles[0]) - 400.0).abs() < 0.01,
        "no internal edge: the union is exactly A, 400 pt²"
    );
}

#[test]
fn faces_remove_builds_the_complement() {
    let (mut doc, elements) = two_squares();
    // Erase the overlap: what is left is the two L-shapes, one object
    // with two contours.
    let after = assert_one_undo_restores(
        &mut doc,
        Operation::PathfinderFaces {
            elements,
            faces: vec!["0-1#0".to_string()],
            mode: FaceSelectMode::Remove,
        },
    );
    assert_eq!(after.rectangles.len(), 1);
    assert_eq!(
        after.rectangles[0].starts.len(),
        2,
        "two disjoint L-shapes ⇒ two contours"
    );
    assert!((polygon_area(&after.rectangles[0]) - 600.0).abs() < 0.01);
}

#[test]
fn faces_rejects_an_unknown_face_id() {
    let (mut doc, elements) = two_squares();
    let before = snapshot(&doc);
    let err = apply(
        &mut doc,
        &Operation::PathfinderFaces {
            elements,
            faces: vec!["7-9#3".to_string()],
            mode: FaceSelectMode::Keep,
        },
    )
    .expect_err("unknown face id must be refused");
    assert!(format!("{err:?}").contains("no face"), "{err:?}");
    assert_eq!(snapshot(&doc), before, "a refused op mutates nothing");
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

#[test]
fn region_verbs_reject_more_inputs_than_the_cap() {
    let (mut doc, elements) = two_squares();
    let mut many = elements.clone();
    for i in 0..20 {
        many.push(NodeId::Rectangle(format!("ghost{i}")));
    }
    let before = snapshot(&doc);
    let err = apply(
        &mut doc,
        &Operation::PathfinderRegion {
            elements: many,
            verb: PathfinderRegionVerb::Divide,
        },
    )
    .expect_err("past the cap the op refuses");
    assert!(format!("{err:?}").contains("at most 12"), "{err:?}");
    assert_eq!(snapshot(&doc), before, "a refused op mutates nothing");
}

#[test]
fn region_verbs_reject_a_duplicated_input() {
    let (mut doc, elements) = two_squares();
    let dup = vec![elements[0].clone(), elements[0].clone()];
    let err = apply(
        &mut doc,
        &Operation::PathfinderRegion {
            elements: dup,
            verb: PathfinderRegionVerb::Divide,
        },
    )
    .expect_err("a duplicated input is a caller bug");
    assert!(format!("{err:?}").contains("twice"), "{err:?}");
}

#[test]
fn region_verbs_reject_inputs_from_different_spreads() {
    let (mut doc, elements) = two_squares();
    let other = doc.spreads[1].spread.rectangles[0]
        .self_id
        .clone()
        .expect("id");
    let err = apply(
        &mut doc,
        &Operation::PathfinderRegion {
            elements: vec![elements[0].clone(), NodeId::Rectangle(other)],
            verb: PathfinderRegionVerb::Divide,
        },
    )
    .expect_err("an arrangement spans one spread");
    assert!(format!("{err:?}").contains("same spread"), "{err:?}");
}

#[test]
fn divide_redo_reproduces_the_result() {
    let (mut doc, elements) = two_squares();
    let op = Operation::PathfinderRegion {
        elements,
        verb: PathfinderRegionVerb::Divide,
    };
    let applied = apply(&mut doc, &op).expect("forward");
    let after = snapshot(&doc);
    let undone = apply(&mut doc, &applied.inverse).expect("undo");
    apply(&mut doc, &undone.inverse).expect("redo");
    assert_eq!(snapshot(&doc), after, "redo reproduces the result exactly");
}
