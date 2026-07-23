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
    let mut doc = paged_parse::import_idml_doc(&fixture_bytes()).expect("open");
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
    let mut doc = paged_parse::import_idml_doc(&fixture_bytes()).expect("open");
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

    let mut doc = paged_parse::import_idml_doc(&fixture_bytes()).expect("open");
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
    let mut doc = paged_parse::import_idml_doc(&fixture_bytes()).expect("open");
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
    let mut doc = paged_parse::import_idml_doc(&fixture_bytes()).expect("open");
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

    let mut doc = paged_parse::import_idml_doc(&fixture_bytes()).expect("open");
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
    let mut doc = paged_parse::import_idml_doc(&strokes_fills_bytes()).expect("open");
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
    let mut doc = paged_parse::import_idml_doc(&strokes_fills_bytes()).expect("open");
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
