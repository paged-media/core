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

//! Pathfinder region verbs against Adobe Illustrator 30.1.0's own
//! output (the paged.draw oracle, `pathfinder-region.illustrator.json`,
//! recorded 2026-10-02: inputs grouped back-to-front, then "Live
//! Pathfinder …" and "expandStyle"). The inputs and the measured
//! results are copied here so the engine's repository carries its own
//! evidence; page space, y down, points.
//!
//! The plugin sends `elements` TOP-TO-BOTTOM, which is the order used
//! here: the blue shape first (in front), the red one second (behind).

use std::path::PathBuf;

use kurbo::Shape;
use paged_mutate::operation::{NodeSpec, PathAnchorSpec};
use paged_mutate::{apply, NodeId, Operation, PathfinderRegionVerb};
use paged_scene::Document;

const RED: &str = "Color/OracleRed";
const BLUE: &str = "Color/OracleBlue";

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

type Point = [f32; 2];

fn corner(p: Point) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: p,
        left: p,
        right: p,
    }
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> Vec<PathAnchorSpec> {
    vec![
        corner([l, t]),
        corner([r, t]),
        corner([r, b]),
        corner([l, b]),
    ]
}

/// The oracle's circle: centre (160, 160), radius 50, four cubics, as
/// Illustrator wrote it (κ·r = 27.614237).
fn circle() -> Vec<PathAnchorSpec> {
    let a = |anchor: Point, left: Point, right: Point| PathAnchorSpec {
        anchor,
        left,
        right,
    };
    vec![
        a([210.0, 160.0], [210.0, 132.385_76], [210.0, 187.614_24]),
        a([160.0, 210.0], [187.614_24, 210.0], [132.385_76, 210.0]),
        a([110.0, 160.0], [110.0, 187.614_24], [110.0, 132.385_76]),
        a([160.0, 110.0], [132.385_76, 110.0], [187.614_24, 110.0]),
    ]
}

/// Append a filled polygon (on top); returns it.
fn add(doc: &mut Document, id: &str, anchors: Vec<PathAnchorSpec>, fill: &str) -> NodeId {
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
                subpath_open: vec![false],
                fill_color: Some(fill.to_string()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
        },
    )
    .unwrap_or_else(|e| panic!("insert {id}: {e:?}"));
    NodeId::Polygon(id.to_string())
}

/// `(fill, area, [min x, min y, max x, max y])` of every polygon the
/// op left whose id is one of `ids` — exact for the Beziers.
fn results(doc: &Document, ids: &[&str]) -> Vec<(String, f64, [f64; 4])> {
    let p = |t: (f32, f32)| kurbo::Point::new(f64::from(t.0), f64::from(t.1));
    doc.spreads[0]
        .spread
        .polygons
        .iter()
        .filter(|poly| ids.contains(&poly.self_id.as_deref().unwrap_or("")))
        .map(|poly| {
            let mut path = kurbo::BezPath::new();
            let starts: Vec<usize> = if poly.subpath_starts.is_empty() {
                vec![0]
            } else {
                poly.subpath_starts.clone()
            };
            for (si, &s) in starts.iter().enumerate() {
                let e = starts.get(si + 1).copied().unwrap_or(poly.anchors.len());
                let run = &poly.anchors[s..e];
                path.move_to(p(run[0].anchor));
                for i in 0..run.len() {
                    let (a, b) = (run[i], run[(i + 1) % run.len()]);
                    path.curve_to(p(a.right), p(b.left), p(b.anchor));
                }
                path.close_path();
            }
            let b = path.bounding_box();
            (
                poly.fill_color.clone().unwrap_or_default(),
                path.area().abs(),
                [b.x0, b.y0, b.x1, b.y1],
            )
        })
        .collect()
}

fn minus_back(doc: &mut Document, top_to_bottom: Vec<NodeId>) {
    apply(
        doc,
        &Operation::PathfinderRegion {
            elements: top_to_bottom,
            verb: PathfinderRegionVerb::MinusBack,
        },
    )
    .expect("minus back");
}

#[track_caller]
fn assert_result(got: &[(String, f64, [f64; 4])], fill: &str, area: f64, bounds: [f64; 4]) {
    assert_eq!(
        got.len(),
        1,
        "Minus Back leaves exactly one object: {got:?}"
    );
    let (f, a, b) = &got[0];
    assert_eq!(
        f, fill,
        "the result keeps the FRONT object's paint: {got:?}"
    );
    assert!(
        (a - area).abs() < 0.05,
        "area {a} — Illustrator measured {area}: {got:?}"
    );
    for (g, w) in b.iter().zip(bounds) {
        assert!(
            (g - w).abs() < 0.01,
            "bounds {b:?} — Illustrator {bounds:?}"
        );
    }
}

/// `rects-minus-back`: red [100,100,200,180] behind, blue
/// [150,140,260,220] in front. Illustrator keeps the BLUE rectangle with
/// the overlap cut out: 8800 − 2000 = 6800 pt², blue, the blue box. The
/// engine used to answer 6000 pt², red, the red box — Illustrator's
/// Minus FRONT.
#[test]
fn minus_back_keeps_the_front_rectangle_minus_what_is_behind_it() {
    let mut doc = fixture();
    let red = add(&mut doc, "red", rect(100.0, 100.0, 200.0, 180.0), RED);
    let blue = add(&mut doc, "blue", rect(150.0, 140.0, 260.0, 220.0), BLUE);
    minus_back(&mut doc, vec![blue, red]);
    assert_result(
        &results(&doc, &["red", "blue"]),
        BLUE,
        6800.0,
        [150.0, 140.0, 260.0, 220.0],
    );
}

/// `circle-rect-minus-back`: a red circle behind, a blue rectangle
/// [150,130,260,190] in front. Illustrator: 3190.786373 pt², blue,
/// [200.00061, 130, 260, 190]. The engine used to answer 4446.4, red.
#[test]
fn minus_back_keeps_the_front_rectangle_minus_the_circle_behind_it() {
    let mut doc = fixture();
    let red = add(&mut doc, "red", circle(), RED);
    let blue = add(&mut doc, "blue", rect(150.0, 130.0, 260.0, 190.0), BLUE);
    minus_back(&mut doc, vec![blue, red]);
    assert_result(
        &results(&doc, &["red", "blue"]),
        BLUE,
        3_190.786_373,
        [200.000_61, 130.0, 260.0, 190.0],
    );
}

/// Three objects: the front one minus BOTH behind it (Illustrator's
/// "subtracts the objects in back from the frontmost object").
#[test]
fn minus_back_subtracts_every_object_behind_the_front_one() {
    let mut doc = fixture();
    let back = add(&mut doc, "back", rect(100.0, 100.0, 140.0, 300.0), RED);
    let middle = add(&mut doc, "middle", rect(260.0, 100.0, 300.0, 300.0), RED);
    let front = add(&mut doc, "front", rect(100.0, 100.0, 300.0, 300.0), BLUE);
    minus_back(&mut doc, vec![front, middle, back]);
    assert_result(
        &results(&doc, &["back", "middle", "front"]),
        BLUE,
        200.0 * 200.0 - 2.0 * 40.0 * 200.0,
        [140.0, 100.0, 260.0, 300.0],
    );
}
