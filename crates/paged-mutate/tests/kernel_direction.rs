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

//! C-81 — every path kernel answers in ONE stated direction.
//!
//! Illustrator 30.1.0 (the paged.draw oracle): offset path and outline
//! stroke results run CLOCKWISE on the page, every Pathfinder result
//! COUNTER-CLOCKWISE, holes against their outer contour. The engine's
//! results used to run whichever way the construction left them —
//! varying with shape, offset sign, input direction and start point.
//! Each case below is run with the input drawn BOTH ways round and from
//! two start points; the answer must not care.

use std::path::PathBuf;

use paged_mutate::operation::{NodeSpec, PathAnchorSpec};
use paged_mutate::orientation::{turn_of, Turn};
use paged_mutate::{
    apply, NodeId, Operation, PathfinderKind, PathfinderRegionVerb, PropertyPath, Value,
};
use paged_scene::Document;

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

fn corner(x: f32, y: f32) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    }
}

/// A rectangle in each of the four ways it can be written: clockwise or
/// counter-clockwise on the page, starting at the top-left or at the
/// bottom-right corner.
fn rect_variants(l: f32, t: f32, r: f32, b: f32) -> Vec<(&'static str, Vec<PathAnchorSpec>)> {
    let cw = vec![corner(l, t), corner(r, t), corner(r, b), corner(l, b)];
    let ccw = vec![corner(l, t), corner(l, b), corner(r, b), corner(r, t)];
    let rot = |v: &Vec<PathAnchorSpec>| {
        let mut v = v.clone();
        v.rotate_left(2);
        v
    };
    vec![
        ("cw", cw.clone()),
        ("ccw", ccw.clone()),
        ("cw from bottom-right", rot(&cw)),
        ("ccw from bottom-right", rot(&ccw)),
    ]
}

fn add(doc: &mut Document, id: &str, anchors: Vec<PathAnchorSpec>, open: bool) -> NodeId {
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
                bounds: [0.0, 0.0, 0.0, 0.0],
                anchors,
                subpath_starts: vec![0],
                subpath_open: vec![open],
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

/// `(direction, |area|)` of each closed contour of a polygon.
fn contours(doc: &Document, id: &str) -> Vec<(Option<Turn>, f64)> {
    let p = doc.spreads[0]
        .spread
        .polygons
        .iter()
        .find(|p| p.self_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("polygon {id}"));
    let starts: Vec<usize> = if p.subpath_starts.is_empty() {
        vec![0]
    } else {
        p.subpath_starts.clone()
    };
    starts
        .iter()
        .enumerate()
        .filter(|(i, _)| !p.subpath_open.get(*i).copied().unwrap_or(false))
        .map(|(i, &s)| {
            let e = starts.get(i + 1).copied().unwrap_or(p.anchors.len());
            let run = &p.anchors[s..e];
            (
                turn_of(run),
                paged_mutate::orientation::signed_area(run).abs(),
            )
        })
        .collect()
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

#[test]
fn offset_path_answers_clockwise_whichever_way_the_input_ran() {
    for delta in [10.0_f32, -10.0] {
        for join in ["miter", "round", "bevel"] {
            for (how, anchors) in rect_variants(100.0, 100.0, 200.0, 160.0) {
                let mut doc = fixture();
                let node = add(&mut doc, "p", anchors, false);
                apply(
                    &mut doc,
                    &set(
                        &node,
                        PropertyPath::OffsetPath,
                        Value::OffsetPath {
                            delta,
                            join: join.to_string(),
                            miter_limit: 4.0,
                            prev_anchors: None,
                            prev_subpath_starts: None,
                            prev_subpath_open: None,
                        },
                    ),
                )
                .unwrap_or_else(|e| panic!("offset {delta} {join} {how}: {e:?}"));
                let got = contours(&doc, "p");
                assert_eq!(got.len(), 1, "{delta} {join} {how}: {got:?}");
                assert_eq!(
                    got[0].0,
                    Some(Turn::Clockwise),
                    "offset {delta} {join} of a rectangle drawn {how}"
                );
            }
        }
    }
}

#[test]
fn outline_stroke_answers_clockwise_with_its_hole_the_other_way() {
    // An open line: one band, clockwise.
    for reversed in [false, true] {
        let mut pts = vec![
            corner(100.0, 100.0),
            corner(200.0, 100.0),
            corner(200.0, 180.0),
        ];
        if reversed {
            pts.reverse();
        }
        let mut doc = fixture();
        let node = add(&mut doc, "p", pts, true);
        apply(
            &mut doc,
            &set(
                &node,
                PropertyPath::OutlineStroke,
                Value::OutlineStroke {
                    width: 10.0,
                    cap: "butt".to_string(),
                    join: "miter".to_string(),
                    miter_limit: 4.0,
                    prev_anchors: None,
                    prev_subpath_starts: None,
                    prev_subpath_open: None,
                },
            ),
        )
        .expect("outline an open line");
        let got = contours(&doc, "p");
        assert!(!got.is_empty());
        for (turn, _) in &got {
            assert_eq!(
                *turn,
                Some(Turn::Clockwise),
                "open line, reversed={reversed}: {got:?}"
            );
        }
    }
    // A closed rectangle: a ring — the outer contour clockwise, the
    // inner one (the hole) counter-clockwise, whichever way it was
    // drawn.
    for (how, anchors) in rect_variants(100.0, 100.0, 200.0, 160.0) {
        let mut doc = fixture();
        let node = add(&mut doc, "p", anchors, false);
        apply(
            &mut doc,
            &set(
                &node,
                PropertyPath::OutlineStroke,
                Value::OutlineStroke {
                    width: 10.0,
                    cap: "butt".to_string(),
                    join: "miter".to_string(),
                    miter_limit: 4.0,
                    prev_anchors: None,
                    prev_subpath_starts: None,
                    prev_subpath_open: None,
                },
            ),
        )
        .expect("outline a rectangle");
        let mut got = contours(&doc, "p");
        got.sort_by(|a, b| b.1.total_cmp(&a.1));
        assert_eq!(got.len(), 2, "{how}: a ring is two contours: {got:?}");
        assert_eq!(got[0].0, Some(Turn::Clockwise), "{how}: the outer contour");
        assert_eq!(got[1].0, Some(Turn::CounterClockwise), "{how}: the hole");
    }
}

#[test]
fn every_pathfinder_boolean_answers_counter_clockwise() {
    for kind in [
        PathfinderKind::Union,
        PathfinderKind::Subtract,
        PathfinderKind::Intersect,
        PathfinderKind::Exclude,
    ] {
        for (how_a, a) in rect_variants(100.0, 100.0, 200.0, 180.0) {
            for (how_b, b) in rect_variants(150.0, 140.0, 260.0, 220.0)
                .into_iter()
                .take(2)
            {
                let mut doc = fixture();
                let back = add(&mut doc, "a", a.clone(), false);
                let front = add(&mut doc, "b", b, false);
                apply(
                    &mut doc,
                    &Operation::PathfinderBoolean {
                        kept: front,
                        others: vec![back],
                        op_kind: kind,
                    },
                )
                .unwrap_or_else(|e| panic!("{kind:?}: {e:?}"));
                let got = contours(&doc, "b");
                for (turn, _) in &got {
                    assert_eq!(
                        *turn,
                        Some(Turn::CounterClockwise),
                        "{kind:?} of a rectangle drawn {how_a} and one drawn {how_b}: {got:?}"
                    );
                }
            }
        }
    }
}

/// Exclude of two crossing rectangles is two DISJOINT regions, as
/// Illustrator answers (rects-exclude: 6800 and 6000 pt², both
/// counter-clockwise) — not two whole overlapping rectangles that cancel
/// only through opposite windings.
#[test]
fn exclude_answers_disjoint_regions() {
    let mut doc = fixture();
    let back = add(
        &mut doc,
        "a",
        rect_variants(100.0, 100.0, 200.0, 180.0)[0].1.clone(),
        false,
    );
    let front = add(
        &mut doc,
        "b",
        rect_variants(150.0, 140.0, 260.0, 220.0)[0].1.clone(),
        false,
    );
    apply(
        &mut doc,
        &Operation::PathfinderBoolean {
            kept: front,
            others: vec![back],
            op_kind: PathfinderKind::Exclude,
        },
    )
    .expect("exclude");
    let mut got = contours(&doc, "b");
    got.sort_by(|a, b| b.1.total_cmp(&a.1));
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(
        (got[0].1 - 6800.0).abs() < 0.5 && (got[1].1 - 6000.0).abs() < 0.5,
        "{got:?}"
    );
    for (turn, _) in &got {
        assert_eq!(*turn, Some(Turn::CounterClockwise), "{got:?}");
    }
}

/// A subtraction that punches a HOLE: the outer contour runs counter-
/// clockwise and the hole the other way, so non-zero fill still leaves
/// the hole empty.
#[test]
fn a_pathfinder_hole_runs_against_its_outer_contour() {
    for (how, inner) in rect_variants(130.0, 130.0, 170.0, 170.0) {
        let mut doc = fixture();
        let hole = add(&mut doc, "hole", inner, false);
        let outer = add(
            &mut doc,
            "outer",
            rect_variants(100.0, 100.0, 200.0, 200.0)[0].1.clone(),
            false,
        );
        apply(
            &mut doc,
            &Operation::PathfinderBoolean {
                kept: outer,
                others: vec![hole],
                op_kind: PathfinderKind::Subtract,
            },
        )
        .expect("subtract");
        let mut got = contours(&doc, "outer");
        got.sort_by(|a, b| b.1.total_cmp(&a.1));
        assert_eq!(got.len(), 2, "{how}: {got:?}");
        assert_eq!(got[0].0, Some(Turn::CounterClockwise), "{how}: outer");
        assert_eq!(got[1].0, Some(Turn::Clockwise), "{how}: hole");
        assert!((got[0].1 - 10_000.0).abs() < 0.5 && (got[1].1 - 1_600.0).abs() < 0.5);
    }
}

#[test]
fn every_region_verb_answers_counter_clockwise() {
    for verb in [
        PathfinderRegionVerb::Divide,
        PathfinderRegionVerb::Trim,
        PathfinderRegionVerb::Merge,
        PathfinderRegionVerb::Crop,
        PathfinderRegionVerb::MinusBack,
    ] {
        for (how, a) in rect_variants(100.0, 100.0, 200.0, 180.0) {
            let mut doc = fixture();
            let back = add(&mut doc, "a", a, false);
            let front = add(
                &mut doc,
                "b",
                rect_variants(150.0, 140.0, 260.0, 220.0)[1].1.clone(),
                false,
            );
            apply(
                &mut doc,
                &Operation::PathfinderRegion {
                    elements: vec![front, back],
                    verb,
                },
            )
            .unwrap_or_else(|e| panic!("{verb:?}: {e:?}"));
            let ids: Vec<String> = doc.spreads[0]
                .spread
                .polygons
                .iter()
                .filter_map(|p| p.self_id.clone())
                .collect();
            let mut seen = 0;
            for id in ids {
                for (turn, _) in contours(&doc, &id) {
                    seen += 1;
                    assert_eq!(
                        turn,
                        Some(Turn::CounterClockwise),
                        "{verb:?} with the back rectangle drawn {how}: {id}"
                    );
                }
            }
            assert!(seen > 0, "{verb:?} produced a result");
        }
    }
}
