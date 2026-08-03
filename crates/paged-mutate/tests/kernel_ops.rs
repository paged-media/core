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

//! B-05 ops (protocol v30) — `OutlineStroke` / `OffsetPath` /
//! `SimplifyPath` through the apply layer: forward result sanity,
//! BYTEWISE inverse restore of the `(anchors, subpath_starts,
//! subpath_open)` triple, and redo (re-applying the captured op).

use std::path::PathBuf;

use paged_mutate::{apply, NodeId, Operation, PropertyPath, Value};
use paged_scene::Document;

fn fixture_bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml");
    std::fs::read(path).expect("read geometry fixture")
}

/// First polygon with a non-empty anchor table (the kernel targets).
fn first_polygon(doc: &Document) -> String {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .filter_map(|p| {
            let id = p.self_id.clone()?;
            (!p.anchors.is_empty()).then_some(id)
        })
        .next()
        .expect("fixture has a polygon with anchors")
}

type AnchorKey = ((f32, f32), (f32, f32), (f32, f32));

/// `paged_model::PathAnchor` derives no `PartialEq` — compare via
/// tuple keys.
fn anchors_of(doc: &Document, id: &str) -> Vec<AnchorKey> {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .find(|p| p.self_id.as_deref() == Some(id))
        .expect("polygon present")
        .anchors
        .iter()
        .map(|a| (a.anchor, a.left, a.right))
        .collect()
}

fn assert_round_trip(doc: &mut Document, id: &str, op: Operation) {
    let before = anchors_of(doc, id);
    let applied = apply(doc, &op).expect("forward apply");
    let after = anchors_of(doc, id);
    assert_ne!(
        before.len(),
        0,
        "fixture polygon must carry anchors before the op"
    );
    assert_ne!(before, after, "op must change the path");
    // Inverse: bytewise restore.
    let undone = apply(doc, &applied.inverse).expect("inverse apply");
    assert_eq!(anchors_of(doc, id), before, "inverse restores bytewise");
    // Redo: the captured forward op re-applies to the same result.
    apply(doc, &undone.inverse).expect("redo apply");
    assert_eq!(anchors_of(doc, id), after, "redo reproduces the result");
}

#[test]
fn outline_stroke_round_trips() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    assert_round_trip(
        &mut doc,
        &id,
        Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::OutlineStroke,
            value: Value::OutlineStroke {
                width: 4.0,
                cap: "butt".to_string(),
                join: "miter".to_string(),
                miter_limit: 4.0,
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        },
    );
}

#[test]
fn outline_stroke_variable_round_trips_or_rejects_cleanly() {
    // B-08 — the variable-width outline op through the apply layer. The
    // kernel math (taper + closed contour) is unit-tested on an open
    // line in `kurbo_kernel`; here we pin the APPLY contract: the new
    // `Value::OutlineStrokeVariable` dispatches, and on a path the v1
    // kernel accepts it round-trips bytewise (forward changes the path,
    // inverse restores, redo reproduces), while on a path it rejects by
    // design (multi-subpath — the fixture polygon is `[0,4]`) it returns
    // a clean `InvalidValue`, never a panic or silent corruption. (Same
    // accept-or-reject-cleanly shape as `offset_path_…` below.)
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let op = Operation::SetProperty {
        node: NodeId::Polygon(id.clone()),
        path: PropertyPath::OutlineStrokeVariable,
        value: Value::OutlineStrokeVariable {
            widths: vec![1.0, 6.0, 2.0],
            cap: "butt".to_string(),
            join: "miter".to_string(),
            miter_limit: 4.0,
            prev_anchors: None,
            prev_subpath_starts: None,
            prev_subpath_open: None,
        },
    };
    let before = anchors_of(&doc, &id);
    match apply(&mut doc, &op) {
        Ok(applied) => {
            assert_ne!(anchors_of(&doc, &id), before, "forward changes the path");
            apply(&mut doc, &applied.inverse).expect("inverse apply");
            assert_eq!(anchors_of(&doc, &id), before, "inverse restores bytewise");
        }
        Err(e) => {
            assert_eq!(anchors_of(&doc, &id), before, "rejection mutates nothing");
            let msg = format!("{e:?}");
            assert!(
                msg.contains("InvalidValue") || msg.contains("kernel"),
                "clean validation error, got: {msg}"
            );
        }
    }
}

#[test]
fn simplify_path_removes_a_redundant_anchor_and_round_trips() {
    use paged_mutate::operation::PathAnchorSpec;

    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let minimal = anchors_of(&doc, &id);

    // Enrich: a collinear anchor at the outer edge's midpoint via the
    // existing PathPointInsert op (the redundancy simplify removes).
    let mid = (
        (minimal[0].0 .0 + minimal[1].0 .0) / 2.0,
        (minimal[0].0 .1 + minimal[1].0 .1) / 2.0,
    );
    apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::PathPointInsert,
            value: Value::PathPointInsert {
                index: 1,
                anchor: PathAnchorSpec {
                    anchor: [mid.0, mid.1],
                    left: [mid.0, mid.1],
                    right: [mid.0, mid.1],
                },
                prev_subpath_starts: None,
            },
        },
    )
    .expect("insert redundant anchor");
    let enriched = anchors_of(&doc, &id);
    assert_eq!(enriched.len(), minimal.len() + 1);

    // Simplify: the collinear anchor goes; inverse restores the
    // ENRICHED state bytewise.
    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::SimplifyPath,
            value: Value::SimplifyPath {
                tolerance: 0.5,
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        },
    )
    .expect("simplify");
    assert!(
        anchors_of(&doc, &id).len() < enriched.len(),
        "redundant anchor removed"
    );
    apply(&mut doc, &applied.inverse).expect("inverse");
    assert_eq!(anchors_of(&doc, &id), enriched, "inverse restores bytewise");
}

#[test]
fn offset_path_round_trips_or_rejects_cleanly() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let op = Operation::SetProperty {
        node: NodeId::Polygon(id.clone()),
        path: PropertyPath::OffsetPath,
        value: Value::OffsetPath {
            delta: 3.0,
            join: "miter".to_string(),
            miter_limit: 4.0,
            prev_anchors: None,
            prev_subpath_starts: None,
            prev_subpath_open: None,
        },
    };
    // The fixture polygon may be open or multi-subpath (the kernel
    // rejects those by design with InvalidValue) — both outcomes are
    // contract-conformant; a PANIC or silent corruption is not.
    let before = anchors_of(&doc, &id);
    match apply(&mut doc, &op) {
        Ok(applied) => {
            assert_ne!(anchors_of(&doc, &id), before);
            apply(&mut doc, &applied.inverse).expect("inverse");
            assert_eq!(anchors_of(&doc, &id), before, "inverse restores");
        }
        Err(e) => {
            assert_eq!(anchors_of(&doc, &id), before, "rejection mutates nothing");
            let msg = format!("{e:?}");
            assert!(
                msg.contains("InvalidValue") || msg.contains("kernel"),
                "clean validation error, got: {msg}"
            );
        }
    }
}

#[test]
fn unknown_join_is_a_clean_invalid_value() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let err = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id),
            path: PropertyPath::OutlineStroke,
            value: Value::OutlineStroke {
                width: 4.0,
                cap: "butt".to_string(),
                join: "zigzag".to_string(),
                miter_limit: 4.0,
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        },
    )
    .expect_err("unknown join must be rejected");
    assert!(format!("{err:?}").contains("zigzag"));
}

/// B-22 — a Polygon (what `insertPath` mints) accepts fill/stroke
/// SetProperty writes instead of rejecting them as "not supported",
/// so a draw plugin no longer has to abuse `setDocumentDefaults` to
/// colour the path it just inserted. Forward changes the field;
/// inverse restores the previous value exactly.
#[test]
fn polygon_fill_and_stroke_set_property_round_trips() {
    fn polygon_fill(doc: &Document, id: &str) -> Option<String> {
        doc.spreads
            .iter()
            .flat_map(|s| s.spread.polygons.iter())
            .find(|p| p.self_id.as_deref() == Some(id))
            .expect("polygon present")
            .fill_color
            .clone()
    }

    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let before = polygon_fill(&doc, &id);

    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some("Color/PagedDrawTest".to_string())),
        },
    )
    .expect("Polygon must accept FrameFillColor (B-22), not reject it");
    assert_eq!(
        polygon_fill(&doc, &id),
        Some("Color/PagedDrawTest".to_string()),
        "forward write lands on the polygon's own fill_color"
    );
    assert_ne!(polygon_fill(&doc, &id), before, "the op changed the fill");

    apply(&mut doc, &applied.inverse).expect("inverse apply");
    assert_eq!(
        polygon_fill(&doc, &id),
        before,
        "inverse restores the previous fill exactly"
    );

    // Stroke colour + weight dispatch on the same kind, too.
    apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::FrameStrokeWeight,
            value: Value::Length(Some(3.5)),
        },
    )
    .expect("Polygon must accept FrameStrokeWeight (B-22)");
}

// ---------------------------------------------------------------------------
// REGRESSION (open finding) — outlineStroke / offsetPath on a PRIMITIVE
// rectangle. An editor-created rectangle (insertFrame) carries bounds but
// EMPTY anchors (the renderer draws it straight from bounds); the path
// kernels used to reject that ("kernel produced no result"). The apply
// layer now synthesizes the rectangle path from the frame bounds, so the
// op succeeds; undo restores the primitive (empty-anchor) rectangle.
// ---------------------------------------------------------------------------

fn strokes_fills_bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("strokes-fills.idml");
    std::fs::read(path).expect("read strokes-fills fixture")
}

fn first_rectangle(doc: &Document) -> String {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .filter_map(|r| r.self_id.clone())
        .next()
        .expect("fixture has a rectangle")
}

fn rect_anchors_of(doc: &Document, id: &str) -> Vec<AnchorKey> {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(id))
        .expect("rectangle present")
        .anchors
        .iter()
        .map(|a| (a.anchor, a.left, a.right))
        .collect()
}

/// Strip a rectangle's anchors in place, leaving only its bounds — the
/// exact shape an editor-created (insertFrame) rectangle parses to.
fn make_primitive_rect(doc: &mut Document, id: &str) {
    for s in doc.spreads.iter_mut() {
        if let Some(r) = s
            .spread
            .rectangles
            .iter_mut()
            .find(|r| r.self_id.as_deref() == Some(id))
        {
            r.anchors.clear();
            r.subpath_starts.clear();
            r.subpath_open.clear();
            return;
        }
    }
    panic!("rectangle {id} not found");
}

#[test]
fn outline_stroke_synthesizes_rect_from_bounds_for_a_primitive_rectangle() {
    let mut doc = idml_import::import_idml_doc(&strokes_fills_bytes()).expect("open");
    let id = first_rectangle(&doc);
    make_primitive_rect(&mut doc, &id);
    assert!(
        rect_anchors_of(&doc, &id).is_empty(),
        "primitive rectangle starts with empty anchors"
    );

    let op = Operation::SetProperty {
        node: NodeId::Rectangle(id.clone()),
        path: PropertyPath::OutlineStroke,
        value: Value::OutlineStroke {
            width: 4.0,
            cap: "butt".to_string(),
            join: "miter".to_string(),
            miter_limit: 4.0,
            prev_anchors: None,
            prev_subpath_starts: None,
            prev_subpath_open: None,
        },
    };
    // Previously rejected; now applies by synthesizing the rect from bounds.
    let applied = apply(&mut doc, &op).expect("outlineStroke on a primitive rectangle applies");
    let after = rect_anchors_of(&doc, &id);
    assert!(!after.is_empty(), "stroke outline produced geometry");

    // Undo restores the primitive rectangle (empty anchors), not a 4-corner path.
    let undone = apply(&mut doc, &applied.inverse).expect("inverse apply");
    assert!(
        rect_anchors_of(&doc, &id).is_empty(),
        "inverse restores the primitive rectangle verbatim"
    );
    // Redo reproduces the outlined result.
    apply(&mut doc, &undone.inverse).expect("redo apply");
    assert_eq!(
        rect_anchors_of(&doc, &id),
        after,
        "redo reproduces the outline"
    );
}

#[test]
fn offset_path_synthesizes_rect_from_bounds_for_a_primitive_rectangle() {
    let mut doc = idml_import::import_idml_doc(&strokes_fills_bytes()).expect("open");
    let id = first_rectangle(&doc);
    make_primitive_rect(&mut doc, &id);
    assert!(rect_anchors_of(&doc, &id).is_empty());

    let op = Operation::SetProperty {
        node: NodeId::Rectangle(id.clone()),
        path: PropertyPath::OffsetPath,
        value: Value::OffsetPath {
            delta: 6.0,
            join: "miter".to_string(),
            miter_limit: 4.0,
            prev_anchors: None,
            prev_subpath_starts: None,
            prev_subpath_open: None,
        },
    };
    let applied = apply(&mut doc, &op).expect("offsetPath on a primitive rectangle applies");
    assert!(
        !rect_anchors_of(&doc, &id).is_empty(),
        "the closed-rect offset produced geometry"
    );
    apply(&mut doc, &applied.inverse).expect("inverse apply");
    assert!(
        rect_anchors_of(&doc, &id).is_empty(),
        "inverse restores the primitive rectangle"
    );
}

// ---------------------------------------------------------------------------
// Wave B (protocol v56) — ClosePath + JoinPaths through the apply layer:
// forward semantics, snapshot-inverse restore, redo, and honest rejection.
// ---------------------------------------------------------------------------

fn poly_tables_of(doc: &Document, id: &str) -> (Vec<usize>, Vec<bool>) {
    let p = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .find(|p| p.self_id.as_deref() == Some(id))
        .expect("polygon present");
    (p.subpath_starts.clone(), p.subpath_open.clone())
}

fn polygon_exists(doc: &Document, id: &str) -> bool {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .any(|p| p.self_id.as_deref() == Some(id))
}

/// Insert a fresh single-contour OPEN polygon (corner anchors at `pts`)
/// on the document's first spread, mirroring what the Pencil tool's
/// `InsertPath` mutation mints.
fn insert_open_poly(doc: &mut Document, id: &str, pts: &[(f32, f32)]) {
    use paged_mutate::operation::PathAnchorSpec;
    let anchors: Vec<PathAnchorSpec> = pts
        .iter()
        .map(|&(x, y)| PathAnchorSpec {
            anchor: [x, y],
            left: [x, y],
            right: [x, y],
        })
        .collect();
    let (mut top, mut left, mut bottom, mut right) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in pts {
        left = left.min(x);
        right = right.max(x);
        top = top.min(y);
        bottom = bottom.max(y);
    }
    let spread_id = doc.spreads[0]
        .spread
        .self_id
        .clone()
        .expect("spread has a self id");
    let position = doc.spreads[0].spread.polygons.len();
    apply(
        &mut *doc,
        &Operation::InsertNode {
            parent: NodeId::Spread(spread_id),
            position,
            node: paged_mutate::NodeSpec::Polygon {
                self_id: id.to_string(),
                bounds: [top, left, bottom, right],
                anchors,
                subpath_starts: vec![0],
                subpath_open: vec![true],
                fill_color: None,
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(1.0),
                item_transform: None,
            },
            z_slot: None,
        },
    )
    .expect("insert open polygon");
}

/// ClosePath is the faithful inverse gesture of PathOpenAt: a scissors
/// cut leaves coincident endpoint twins, and closing merges them back
/// (same anchor count as before the cut). Undo restores the OPEN state
/// bytewise (reopens at the same point); redo re-closes.
#[test]
fn close_path_reverses_a_scissors_cut_and_round_trips() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let closed_anchors = anchors_of(&doc, &id);
    let (closed_starts, closed_open) = poly_tables_of(&doc, &id);
    assert!(
        closed_open.iter().all(|o| !o),
        "fixture polygon starts fully closed"
    );

    // Scissors cut at anchor 1 (subpath 0 opens, coincident twins).
    apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::PathOpenAt,
            value: Value::PathOpenAt {
                index: 1,
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        },
    )
    .expect("scissors cut");
    let open_anchors = anchors_of(&doc, &id);
    let (open_starts, open_flags) = poly_tables_of(&doc, &id);
    assert_eq!(
        open_anchors.len(),
        closed_anchors.len() + 1,
        "cut duplicated the anchor"
    );
    assert!(open_flags[0], "subpath 0 is open after the cut");

    // ClosePath (default subpath = the single open contour) merges the
    // twins back: anchor count returns to the pre-cut count and no
    // contour stays open.
    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::ClosePath,
            value: Value::ClosePath {
                subpath: None,
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        },
    )
    .expect("close path");
    let reclosed_anchors = anchors_of(&doc, &id);
    let (reclosed_starts, reclosed_open) = poly_tables_of(&doc, &id);
    assert_eq!(
        reclosed_anchors.len(),
        closed_anchors.len(),
        "coincident endpoints merged back into one anchor"
    );
    assert!(reclosed_open.iter().all(|o| !o), "no contour stays open");
    assert_eq!(
        reclosed_starts, closed_starts,
        "subpath boundaries return to the pre-cut table"
    );

    // Undo: bytewise restore of the OPEN state (reopen at the same point).
    let undone = apply(&mut doc, &applied.inverse).expect("undo close");
    assert_eq!(anchors_of(&doc, &id), open_anchors, "undo reopens bytewise");
    assert_eq!(
        poly_tables_of(&doc, &id),
        (open_starts, open_flags),
        "undo restores the open tables verbatim"
    );

    // Redo: the closed-merged state again, bytewise.
    apply(&mut doc, &undone.inverse).expect("redo close");
    assert_eq!(
        anchors_of(&doc, &id),
        reclosed_anchors,
        "redo re-closes bytewise"
    );
    assert_eq!(poly_tables_of(&doc, &id), (reclosed_starts, reclosed_open));
}

/// Endpoints APART (the twin was deleted after the cut): ClosePath
/// keeps every anchor and just marks the contour closed — the renderer
/// draws the implicit straight closing edge. Undo restores the open
/// state verbatim.
#[test]
fn close_path_with_gap_endpoints_closes_without_merging() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let node = NodeId::Polygon(id.clone());

    // Cut at anchor 1, then remove the duplicated tail twin so the
    // contour's endpoints are genuinely distinct points.
    apply(
        &mut doc,
        &Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PathOpenAt,
            value: Value::PathOpenAt {
                index: 1,
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        },
    )
    .expect("scissors cut");
    let (starts, _) = poly_tables_of(&doc, &id);
    let subpath0_end = starts
        .get(1)
        .copied()
        .unwrap_or(anchors_of(&doc, &id).len());
    apply(
        &mut doc,
        &Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PathPointRemove,
            value: Value::PathPointRemove {
                index: subpath0_end - 1,
                prev_subpath_starts: None,
            },
        },
    )
    .expect("remove the tail twin");
    let gap_anchors = anchors_of(&doc, &id);
    let gap_tables = poly_tables_of(&doc, &id);
    assert!(gap_tables.1[0], "contour is open with distinct endpoints");

    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::ClosePath,
            value: Value::ClosePath {
                subpath: Some(0),
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        },
    )
    .expect("close path over a gap");
    assert_eq!(
        anchors_of(&doc, &id).len(),
        gap_anchors.len(),
        "no anchor merged — the closing edge is implicit"
    );
    let (_, flags) = poly_tables_of(&doc, &id);
    assert!(!flags[0], "contour 0 is closed");

    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(anchors_of(&doc, &id), gap_anchors, "undo restores bytewise");
    assert_eq!(poly_tables_of(&doc, &id), gap_tables);
}

/// Honest rejections: a fully-closed path has nothing to close
/// (default and explicit-index forms), and an out-of-range subpath
/// index is refused — all without mutating anything.
#[test]
fn close_path_on_a_closed_path_rejects_cleanly() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let node = NodeId::Polygon(id.clone());
    let before = anchors_of(&doc, &id);
    let before_tables = poly_tables_of(&doc, &id);

    let close = |subpath| Operation::SetProperty {
        node: node.clone(),
        path: PropertyPath::ClosePath,
        value: Value::ClosePath {
            subpath,
            prev_anchors: None,
            prev_subpath_starts: None,
            prev_subpath_open: None,
        },
    };
    let err = apply(&mut doc, &close(None)).expect_err("no open subpath");
    assert!(format!("{err:?}").contains("no open subpath"), "{err:?}");
    let err = apply(&mut doc, &close(Some(0))).expect_err("subpath 0 is closed");
    assert!(format!("{err:?}").contains("already closed"), "{err:?}");
    let err = apply(&mut doc, &close(Some(99))).expect_err("index out of range");
    assert!(format!("{err:?}").contains("out of range"), "{err:?}");
    assert_eq!(anchors_of(&doc, &id), before, "rejection mutates nothing");
    assert_eq!(poly_tables_of(&doc, &id), before_tables);
}

/// JoinPaths welds at the NEAREST endpoint pair (here kept.end →
/// other.start, no reversal, no coincidence): the other's anchors
/// append onto the kept contour, the other element disappears, and
/// ONE undo restores both elements exactly.
#[test]
fn join_paths_welds_nearest_endpoints_and_one_undo_restores_both() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    insert_open_poly(&mut doc, "wbJoinA", &[(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)]);
    insert_open_poly(
        &mut doc,
        "wbJoinB",
        &[(20.0, 10.0), (30.0, 10.0), (40.0, 10.0)],
    );
    let a_before = anchors_of(&doc, "wbJoinA");
    let b_before = anchors_of(&doc, "wbJoinB");
    let a_tables = poly_tables_of(&doc, "wbJoinA");

    let applied = apply(
        &mut doc,
        &Operation::JoinPaths {
            kept: NodeId::Polygon("wbJoinA".to_string()),
            other: NodeId::Polygon("wbJoinB".to_string()),
        },
    )
    .expect("join");
    assert!(!polygon_exists(&doc, "wbJoinB"), "other element removed");
    let joined = anchors_of(&doc, "wbJoinA");
    assert_eq!(joined.len(), 6, "kept + other anchors, no merge");
    // Sequence: kept in order, then other in order (weld = ke→os).
    let xs: Vec<f32> = joined.iter().map(|a| a.0 .0).collect();
    assert_eq!(xs, vec![0.0, 10.0, 20.0, 20.0, 30.0, 40.0]);
    let (starts, flags) = poly_tables_of(&doc, "wbJoinA");
    assert_eq!(starts, vec![0], "one contour");
    assert_eq!(flags, vec![true], "endpoints apart: the weld stays open");

    // ONE undo restores BOTH elements exactly.
    let undone = apply(&mut doc, &applied.inverse).expect("undo join");
    assert_eq!(
        anchors_of(&doc, "wbJoinA"),
        a_before,
        "kept path restored bytewise"
    );
    assert_eq!(poly_tables_of(&doc, "wbJoinA"), a_tables);
    assert!(polygon_exists(&doc, "wbJoinB"), "other element re-inserted");
    assert_eq!(
        anchors_of(&doc, "wbJoinB"),
        b_before,
        "other geometry restored bytewise"
    );
    let (_, b_flags) = poly_tables_of(&doc, "wbJoinB");
    assert_eq!(b_flags, vec![true], "other restored as an open path");

    // Redo replays the join deterministically.
    apply(&mut doc, &undone.inverse).expect("redo join");
    assert!(!polygon_exists(&doc, "wbJoinB"));
    assert_eq!(
        anchors_of(&doc, "wbJoinA"),
        joined,
        "redo reproduces the weld"
    );
}

/// The other contour reverses when ITS far end is the nearest one, so
/// the welded sequence runs continuously through the joint.
#[test]
fn join_paths_reverses_orientation_to_meet_nearest_endpoints() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    insert_open_poly(&mut doc, "wbRevA", &[(0.0, 0.0), (10.0, 0.0)]);
    // B runs BACKWARDS: its END (12,0) is nearest A's end (10,0).
    insert_open_poly(&mut doc, "wbRevB", &[(30.0, 0.0), (12.0, 0.0)]);

    apply(
        &mut doc,
        &Operation::JoinPaths {
            kept: NodeId::Polygon("wbRevA".to_string()),
            other: NodeId::Polygon("wbRevB".to_string()),
        },
    )
    .expect("join");
    let xs: Vec<f32> = anchors_of(&doc, "wbRevA").iter().map(|a| a.0 .0).collect();
    assert_eq!(
        xs,
        vec![0.0, 10.0, 12.0, 30.0],
        "other contour reversed so the weld runs continuously"
    );
}

/// BOTH endpoint pairs coincident = the two halves of a cut ring:
/// joining merges both pairs and closes the contour.
#[test]
fn join_paths_coincident_ring_closes() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    // Two halves of the unit-square outline sharing both endpoints.
    insert_open_poly(
        &mut doc,
        "wbRingA",
        &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
    );
    insert_open_poly(
        &mut doc,
        "wbRingB",
        &[(10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
    );

    let applied = apply(
        &mut doc,
        &Operation::JoinPaths {
            kept: NodeId::Polygon("wbRingA".to_string()),
            other: NodeId::Polygon("wbRingB".to_string()),
        },
    )
    .expect("join ring");
    let ring = anchors_of(&doc, "wbRingA");
    assert_eq!(ring.len(), 4, "both coincident pairs merged");
    let (starts, flags) = poly_tables_of(&doc, "wbRingA");
    assert_eq!(starts, vec![0]);
    assert_eq!(flags, vec![false], "the ring is CLOSED");
    let corners: Vec<(f32, f32)> = ring.iter().map(|a| a.0).collect();
    assert_eq!(
        corners,
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
        "the square ring, one anchor per corner"
    );

    // Undo restores both open halves.
    apply(&mut doc, &applied.inverse).expect("undo ring join");
    assert!(polygon_exists(&doc, "wbRingB"));
    let (_, a_flags) = poly_tables_of(&doc, "wbRingA");
    assert_eq!(a_flags, vec![true], "kept half is open again");
    assert_eq!(anchors_of(&doc, "wbRingA").len(), 3);
    assert_eq!(anchors_of(&doc, "wbRingB").len(), 3);
}

/// Honest rejections: closed inputs, self-joins, and non-path nodes
/// refuse with a clean error and mutate nothing.
#[test]
fn join_paths_rejects_closed_self_and_non_path_inputs() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let closed_id = first_polygon(&doc);
    insert_open_poly(&mut doc, "wbOpen", &[(0.0, 0.0), (10.0, 0.0)]);
    let closed_before = anchors_of(&doc, &closed_id);
    let open_before = anchors_of(&doc, "wbOpen");

    // Closed kept input.
    let err = apply(
        &mut doc,
        &Operation::JoinPaths {
            kept: NodeId::Polygon(closed_id.clone()),
            other: NodeId::Polygon("wbOpen".to_string()),
        },
    )
    .expect_err("closed kept input");
    let msg = format!("{err:?}");
    assert!(msg.contains("closed") || msg.contains("contours"), "{msg}");

    // Closed other input.
    let err = apply(
        &mut doc,
        &Operation::JoinPaths {
            kept: NodeId::Polygon("wbOpen".to_string()),
            other: NodeId::Polygon(closed_id.clone()),
        },
    )
    .expect_err("closed other input");
    let msg = format!("{err:?}");
    assert!(msg.contains("closed") || msg.contains("contours"), "{msg}");

    // Self-join.
    let err = apply(
        &mut doc,
        &Operation::JoinPaths {
            kept: NodeId::Polygon("wbOpen".to_string()),
            other: NodeId::Polygon("wbOpen".to_string()),
        },
    )
    .expect_err("self-join");
    assert!(format!("{err:?}").contains("itself"));

    // Non-path node kind.
    let err = apply(
        &mut doc,
        &Operation::JoinPaths {
            kept: NodeId::Oval("nope".to_string()),
            other: NodeId::Polygon("wbOpen".to_string()),
        },
    )
    .expect_err("non-path kept input");
    assert!(format!("{err:?}").contains("NodeNotFound"), "{err:?}");

    assert_eq!(
        anchors_of(&doc, &closed_id),
        closed_before,
        "nothing mutated"
    );
    assert_eq!(anchors_of(&doc, "wbOpen"), open_before, "nothing mutated");
    assert!(polygon_exists(&doc, "wbOpen"));
}

// ---------------------------------------------------------------------------
// B-18 (protocol v56) — PasteInto + ReleaseFrom through the apply layer:
// nested-children bookkeeping, exact z-slot / child-index restore on undo,
// world-transform preservation, and honest rejection.
// ---------------------------------------------------------------------------

/// Insert a fresh rectangle on the document's first spread.
fn insert_rect(doc: &mut Document, id: &str, bounds: [f32; 4]) {
    let spread_id = doc.spreads[0]
        .spread
        .self_id
        .clone()
        .expect("spread has a self id");
    let position = doc.spreads[0].spread.rectangles.len();
    apply(
        &mut *doc,
        &Operation::InsertNode {
            parent: NodeId::Spread(spread_id),
            position,
            node: paged_mutate::NodeSpec::Rectangle {
                self_id: id.to_string(),
                bounds,
                fill_color: Some("Color/Black".to_string()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]),
            },
            z_slot: None,
        },
    )
    .expect("insert rectangle");
}

/// Resolve the first spread's `frames_in_order` to `Self` ids (for
/// order-sensitive assertions without `FrameRef` index bookkeeping).
fn top_level_ids(doc: &Document) -> Vec<String> {
    let spread = &doc.spreads[0].spread;
    spread
        .frames_in_order
        .iter()
        .filter_map(|r| {
            let id = match *r {
                paged_model::FrameRef::TextFrame(i) => spread.text_frames.get(i)?.self_id.clone(),
                paged_model::FrameRef::Rectangle(i) => spread.rectangles.get(i)?.self_id.clone(),
                paged_model::FrameRef::Oval(i) => spread.ovals.get(i)?.self_id.clone(),
                paged_model::FrameRef::GraphicLine(i) => {
                    spread.graphic_lines.get(i)?.self_id.clone()
                }
                paged_model::FrameRef::Polygon(i) => spread.polygons.get(i)?.self_id.clone(),
                paged_model::FrameRef::Group(i) => spread.groups.get(i)?.self_id.clone(),
            };
            id
        })
        .collect()
}

fn nested_ids_of(doc: &Document, host: &str) -> Vec<String> {
    let spread = &doc.spreads[0].spread;
    spread
        .nested_children
        .get(host)
        .map(|children| {
            children
                .iter()
                .filter_map(|r| match *r {
                    paged_model::FrameRef::TextFrame(i) => {
                        spread.text_frames.get(i)?.self_id.clone()
                    }
                    paged_model::FrameRef::Rectangle(i) => {
                        spread.rectangles.get(i)?.self_id.clone()
                    }
                    paged_model::FrameRef::Oval(i) => spread.ovals.get(i)?.self_id.clone(),
                    paged_model::FrameRef::GraphicLine(i) => {
                        spread.graphic_lines.get(i)?.self_id.clone()
                    }
                    paged_model::FrameRef::Polygon(i) => spread.polygons.get(i)?.self_id.clone(),
                    paged_model::FrameRef::Group(i) => spread.groups.get(i)?.self_id.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn rect_transform(doc: &Document, id: &str) -> Option<[f32; 6]> {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(id))
        .expect("rectangle present")
        .item_transform
}

/// PasteInto moves the child out of the z-table into the container's
/// nested-children list, GEOMETRY PRESERVED IN DOCUMENT SPACE (the
/// composed `item_transform` is untouched), and ONE undo restores the
/// exact stacking slot. Redo re-nests.
#[test]
fn paste_into_nests_a_child_and_one_undo_restores_the_exact_z_slot() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    insert_rect(&mut doc, "b18Host", [0.0, 0.0, 200.0, 300.0]);
    insert_rect(&mut doc, "b18Child", [10.0, 10.0, 50.0, 50.0]);
    insert_rect(&mut doc, "b18Above", [0.0, 0.0, 20.0, 20.0]);
    let order_before = top_level_ids(&doc);
    let t_before = rect_transform(&doc, "b18Child");
    assert!(order_before.contains(&"b18Child".to_string()));

    let applied = apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::Rectangle("b18Host".to_string()),
            child: NodeId::Rectangle("b18Child".to_string()),
            child_index: None,
        },
    )
    .expect("paste into");
    let order_nested = top_level_ids(&doc);
    assert!(
        !order_nested.contains(&"b18Child".to_string()),
        "child left the z-table: {order_nested:?}"
    );
    assert_eq!(nested_ids_of(&doc, "b18Host"), vec!["b18Child".to_string()]);
    assert_eq!(
        rect_transform(&doc, "b18Child"),
        t_before,
        "geometry preserved in document space — the transform is untouched"
    );

    // ONE undo pops the child back to its EXACT prior slot.
    let undone = apply(&mut doc, &applied.inverse).expect("undo paste");
    assert_eq!(top_level_ids(&doc), order_before, "exact z slot restored");
    assert!(nested_ids_of(&doc, "b18Host").is_empty());
    assert_eq!(rect_transform(&doc, "b18Child"), t_before);

    // Redo re-nests.
    apply(&mut doc, &undone.inverse).expect("redo paste");
    assert_eq!(nested_ids_of(&doc, "b18Host"), vec!["b18Child".to_string()]);
    assert_eq!(top_level_ids(&doc), order_nested);
}

/// ReleaseFrom pops the child to the top of the z-table (world
/// transform preserved); its inverse re-nests at the SAME index in
/// the container's child list.
#[test]
fn release_from_pops_the_child_and_undo_renests_at_the_same_index() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    insert_rect(&mut doc, "b18Host", [0.0, 0.0, 200.0, 300.0]);
    insert_rect(&mut doc, "b18A", [10.0, 10.0, 50.0, 50.0]);
    insert_rect(&mut doc, "b18B", [20.0, 20.0, 60.0, 60.0]);
    for child in ["b18A", "b18B"] {
        apply(
            &mut doc,
            &Operation::PasteInto {
                container: NodeId::Rectangle("b18Host".to_string()),
                child: NodeId::Rectangle(child.to_string()),
                child_index: None,
            },
        )
        .expect("paste into");
    }
    assert_eq!(
        nested_ids_of(&doc, "b18Host"),
        vec!["b18A".to_string(), "b18B".to_string()]
    );
    let t_before = rect_transform(&doc, "b18A");

    // Release the FIRST child (index 0) so the inverse must restore a
    // non-trivial child_index.
    let applied = apply(
        &mut doc,
        &Operation::ReleaseFrom {
            child: NodeId::Rectangle("b18A".to_string()),
            restore_slot: None,
        },
    )
    .expect("release");
    let order = top_level_ids(&doc);
    assert_eq!(
        order.last().map(String::as_str),
        Some("b18A"),
        "released on top: {order:?}"
    );
    assert_eq!(nested_ids_of(&doc, "b18Host"), vec!["b18B".to_string()]);
    assert_eq!(rect_transform(&doc, "b18A"), t_before, "no canvas motion");

    // Undo re-nests at index 0 (before b18B), not appended.
    apply(&mut doc, &applied.inverse).expect("undo release");
    assert_eq!(
        nested_ids_of(&doc, "b18Host"),
        vec!["b18A".to_string(), "b18B".to_string()],
        "child_index restored"
    );
}

/// Honest rejections, document untouched: self-paste, missing
/// container, non-container host kind, double-paste, cycles, and a
/// grouped child.
#[test]
fn paste_into_rejections_leave_the_document_untouched() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    insert_rect(&mut doc, "b18Host", [0.0, 0.0, 200.0, 300.0]);
    insert_rect(&mut doc, "b18Child", [10.0, 10.0, 50.0, 50.0]);
    let order_before = top_level_ids(&doc);

    // Into itself.
    let err = apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::Rectangle("b18Host".to_string()),
            child: NodeId::Rectangle("b18Host".to_string()),
            child_index: None,
        },
    )
    .expect_err("self-paste");
    assert!(format!("{err:?}").contains("itself"), "{err:?}");

    // Missing container.
    let err = apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::Rectangle("nope".to_string()),
            child: NodeId::Rectangle("b18Child".to_string()),
            child_index: None,
        },
    )
    .expect_err("missing container");
    assert!(format!("{err:?}").contains("same spread"), "{err:?}");

    // Non-container host kind (a TextFrame can't host paste-into in
    // this scope).
    let err = apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::TextFrame("b18Host".to_string()),
            child: NodeId::Rectangle("b18Child".to_string()),
            child_index: None,
        },
    )
    .expect_err("text-frame host");
    assert!(format!("{err:?}").contains("InvalidParent"), "{err:?}");

    // Double paste.
    apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::Rectangle("b18Host".to_string()),
            child: NodeId::Rectangle("b18Child".to_string()),
            child_index: None,
        },
    )
    .expect("first paste");
    let err = apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::Rectangle("b18Host".to_string()),
            child: NodeId::Rectangle("b18Child".to_string()),
            child_index: None,
        },
    )
    .expect_err("double paste");
    assert!(format!("{err:?}").contains("already pasted"), "{err:?}");

    // Cycle: host is (transitively) inside child — pasting the host
    // into its own nested child must fail.
    let err = apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::Rectangle("b18Child".to_string()),
            child: NodeId::Rectangle("b18Host".to_string()),
            child_index: None,
        },
    )
    .expect_err("cycle");
    assert!(format!("{err:?}").contains("nest the container"), "{err:?}");

    // Grouped child: a member of the fixture's first group can't be
    // pasted into a frame without ungrouping first.
    let grouped_node = {
        let spread = &doc.spreads[0].spread;
        let grouped_ref = spread.groups[0].members[0];
        match grouped_ref {
            paged_model::FrameRef::Rectangle(i) => {
                spread.rectangles[i].self_id.clone().map(NodeId::Rectangle)
            }
            paged_model::FrameRef::Polygon(i) => {
                spread.polygons[i].self_id.clone().map(NodeId::Polygon)
            }
            paged_model::FrameRef::Oval(i) => spread.ovals[i].self_id.clone().map(NodeId::Oval),
            paged_model::FrameRef::TextFrame(i) => {
                spread.text_frames[i].self_id.clone().map(NodeId::TextFrame)
            }
            paged_model::FrameRef::GraphicLine(i) => spread.graphic_lines[i]
                .self_id
                .clone()
                .map(NodeId::GraphicLine),
            paged_model::FrameRef::Group(_) => None,
        }
    };
    if let Some(grouped_node) = grouped_node {
        let err = apply(
            &mut doc,
            &Operation::PasteInto {
                container: NodeId::Rectangle("b18Host".to_string()),
                child: grouped_node,
                child_index: None,
            },
        )
        .expect_err("grouped child");
        assert!(format!("{err:?}").contains("ungroup first"), "{err:?}");
    }

    // Untouched check: the child is nested exactly once from the one
    // successful paste above; everything else identical.
    assert_eq!(nested_ids_of(&doc, "b18Host"), vec!["b18Child".to_string()]);
    let mut expect = order_before;
    expect.retain(|id| id != "b18Child");
    assert_eq!(top_level_ids(&doc), expect);
}

/// RemoveNode of a nested child is rejected (its inverse would
/// restore the child top-level, breaking undo identity); after a
/// release, the removal goes through.
#[test]
fn remove_node_of_a_nested_child_requires_release_first() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    insert_rect(&mut doc, "b18Host", [0.0, 0.0, 200.0, 300.0]);
    insert_rect(&mut doc, "b18Child", [10.0, 10.0, 50.0, 50.0]);
    apply(
        &mut doc,
        &Operation::PasteInto {
            container: NodeId::Rectangle("b18Host".to_string()),
            child: NodeId::Rectangle("b18Child".to_string()),
            child_index: None,
        },
    )
    .expect("paste");

    let err = apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("b18Child".to_string()),
        },
    )
    .expect_err("remove nested child");
    assert!(format!("{err:?}").contains("release"), "{err:?}");
    assert_eq!(nested_ids_of(&doc, "b18Host"), vec!["b18Child".to_string()]);

    apply(
        &mut doc,
        &Operation::ReleaseFrom {
            child: NodeId::Rectangle("b18Child".to_string()),
            restore_slot: None,
        },
    )
    .expect("release");
    apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::Rectangle("b18Child".to_string()),
        },
    )
    .expect("remove after release");
    assert!(nested_ids_of(&doc, "b18Host").is_empty());
}

// ── B-23 — corner option / radius on a Polygon ───────────────────────

/// Read a polygon's `(option, radius)` for one corner slot.
fn poly_corner(
    doc: &Document,
    id: &str,
    i: usize,
) -> (Option<paged_model::CornerOption>, Option<f32>) {
    let p = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .find(|p| p.self_id.as_deref() == Some(id))
        .expect("polygon present");
    (p.corners[i].option, p.corners[i].radius)
}

/// B-23 — `frameCornerOption*` / `frameCornerRadius*` now apply to a
/// Polygon, not just a Rectangle. All four named slots are writable
/// (they round-trip through IDML); the geometry decision that only
/// `TopLeft` DRIVES the polygon's outline lives in the renderer, so the
/// mutation surface stays symmetric with the Rectangle arms.
#[test]
fn polygon_corner_option_and_radius_apply_and_undo_bytewise() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);

    // Baseline: the fixture polygon carries no corner attributes.
    assert_eq!(poly_corner(&doc, &id, 0), (None, None));

    let set_option = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::FrameCornerOptionTopLeft,
            value: Value::Text("RoundedCorner".to_string()),
        },
    )
    .expect("polygon corner option is applicable");
    assert_eq!(
        poly_corner(&doc, &id, 0).0,
        Some(paged_model::CornerOption::Rounded)
    );

    let set_radius = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Polygon(id.clone()),
            path: PropertyPath::FrameCornerRadiusTopLeft,
            value: Value::Length(Some(24.0)),
        },
    )
    .expect("polygon corner radius is applicable");
    assert_eq!(
        poly_corner(&doc, &id, 0),
        (Some(paged_model::CornerOption::Rounded), Some(24.0))
    );

    // Undo the radius, then the option — each restores the prior value
    // bytewise (the inverse captured `None` for both).
    apply(&mut doc, &set_radius.inverse).expect("undo radius");
    assert_eq!(
        poly_corner(&doc, &id, 0),
        (Some(paged_model::CornerOption::Rounded), None)
    );
    apply(&mut doc, &set_option.inverse).expect("undo option");
    assert_eq!(poly_corner(&doc, &id, 0), (None, None));
}

/// Every one of the four named slots is addressable on a Polygon, and
/// the prior-value capture is per-slot (writing TopRight must not
/// disturb TopLeft).
#[test]
fn polygon_corner_slots_are_independent() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let id = first_polygon(&doc);
    let slots = [
        (PropertyPath::FrameCornerRadiusTopLeft, 0usize, 4.0f32),
        (PropertyPath::FrameCornerRadiusTopRight, 1, 5.0),
        (PropertyPath::FrameCornerRadiusBottomRight, 2, 6.0),
        (PropertyPath::FrameCornerRadiusBottomLeft, 3, 7.0),
    ];
    let mut applied = Vec::new();
    for (path, _, v) in slots {
        applied.push(
            apply(
                &mut doc,
                &Operation::SetProperty {
                    node: NodeId::Polygon(id.clone()),
                    path,
                    value: Value::Length(Some(v)),
                },
            )
            .expect("slot applicable"),
        );
    }
    for (_, i, v) in slots {
        assert_eq!(poly_corner(&doc, &id, i).1, Some(v), "slot {i}");
    }
    // Undo in reverse order restores the empty baseline.
    for a in applied.iter().rev() {
        apply(&mut doc, &a.inverse).expect("undo");
    }
    for (_, i, _) in slots {
        assert_eq!(poly_corner(&doc, &id, i), (None, None), "slot {i} restored");
    }
}
