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

/// `paged_parse::PathAnchor` derives no `PartialEq` — compare via
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
    let mut doc = Document::open(&fixture_bytes()).expect("open");
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
fn simplify_path_removes_a_redundant_anchor_and_round_trips() {
    use paged_mutate::operation::PathAnchorSpec;

    let mut doc = Document::open(&fixture_bytes()).expect("open");
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
    let mut doc = Document::open(&fixture_bytes()).expect("open");
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
    let mut doc = Document::open(&fixture_bytes()).expect("open");
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
