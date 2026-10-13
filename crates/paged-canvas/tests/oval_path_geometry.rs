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

//! An oval is a path: the host door `pathAnchors` (`RequestPathAnchors`)
//! answers its four-arc Bézier, and the planar arrangement the shape
//! builder reads (`RequestPlanarRegions`) sees the ellipse, not its box.
//!
//! The model keeps an oval as bounds only, so the door answered nothing
//! ("Oval carries no path geometry") and the arrangement stood a bounding
//! rectangle in for it — the shape builder merged two ellipses into a
//! rectangle. The anchors come back in the element's raw path space with
//! its `itemTransform`, exactly as a polygon's do, spelled the way
//! InDesign spells an oval: edge midpoints bottom → right → top → left,
//! handles κ·r along the tangent.

use paged_canvas::channel::Mutation;
use paged_canvas::element_selection::ElementId;
use paged_canvas::{blank::blank_idml, CanvasModel, CanvasOptions, PageId};

const KAPPA: f32 = 0.552_284_8;

fn model_with_two_ovals() -> (CanvasModel, String, String) {
    let mut model =
        CanvasModel::load("ovals", &blank_idml(612.0, 792.0), CanvasOptions::default()).unwrap();
    let page = model.handle().page_ids[0].clone();
    for bounds in [(100.0, 100.0, 300.0, 300.0), (100.0, 200.0, 300.0, 400.0)] {
        model
            .apply_mutation(&Mutation::InsertOval {
                page_id: PageId(page.0.clone()),
                bounds,
            })
            .expect("insert oval");
    }
    let ids: Vec<String> = model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.ovals.iter())
        .filter_map(|o| o.self_id.clone())
        .collect();
    assert_eq!(ids.len(), 2, "two ovals minted");
    (model, ids[0].clone(), ids[1].clone())
}

fn apply(m: &[f32; 6], (x, y): (f32, f32)) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn close(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
}

#[test]
fn an_oval_answers_its_four_arc_bezier() {
    let (model, a, _) = model_with_two_ovals();
    let oval = model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.ovals.iter())
        .find(|o| o.self_id.as_deref() == Some(a.as_str()))
        .unwrap()
        .clone();
    let table = model
        .path_anchors(&ElementId::Oval(a.clone()))
        .expect("an oval answers pathAnchors");
    assert_eq!(table.anchors.len(), 4, "{:?}", table.anchors);
    assert_eq!(table.item_transform, oval.item_transform);
    assert!(table.page_id.is_some(), "the oval sits on a page");
    // Closed, one contour.
    assert!(table.subpath_starts.len() <= 1);
    assert_eq!(table.subpath_open, vec![false]);

    // In raw path space: the ellipse inscribed in the oval's bounds.
    let b = oval.bounds;
    let (cx, cy) = ((b.left + b.right) * 0.5, (b.top + b.bottom) * 0.5);
    let (kx, ky) = (
        (b.right - b.left) * 0.5 * KAPPA,
        (b.bottom - b.top) * 0.5 * KAPPA,
    );
    let want = [
        ((cx, b.bottom), (cx - kx, b.bottom), (cx + kx, b.bottom)),
        ((b.right, cy), (b.right, cy + ky), (b.right, cy - ky)),
        ((cx, b.top), (cx + kx, b.top), (cx - kx, b.top)),
        ((b.left, cy), (b.left, cy - ky), (b.left, cy + ky)),
    ];
    for (got, (anchor, left, right)) in table.anchors.iter().zip(want) {
        let g = |p: [f32; 2]| (p[0], p[1]);
        assert!(close(g(got.anchor), anchor), "{got:?} vs {anchor:?}");
        assert!(close(g(got.left), left), "{got:?} vs {left:?}");
        assert!(close(g(got.right), right), "{got:?} vs {right:?}");
    }

    // Through its transform it lands where the oval was drawn: the page
    // box 100..300 × 100..300 (the blank page sits at the spread origin).
    let m = table
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let placed: Vec<(f32, f32)> = table
        .anchors
        .iter()
        .map(|p| apply(&m, (p.anchor[0], p.anchor[1])))
        .collect();
    let page_origin = page_origin(&model);
    let local: Vec<(f32, f32)> = placed
        .iter()
        .map(|(x, y)| (x - page_origin.0, y - page_origin.1))
        .collect();
    assert!(close(local[0], (200.0, 300.0)), "{local:?}");
    assert!(close(local[1], (300.0, 200.0)), "{local:?}");
    assert!(close(local[2], (200.0, 100.0)), "{local:?}");
    assert!(close(local[3], (100.0, 200.0)), "{local:?}");
}

/// The page's top-left corner in spread coordinates.
fn page_origin(model: &CanvasModel) -> (f32, f32) {
    let p = &model.scene().spreads[0].spread.pages[0];
    let m = p.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    apply(&m, (p.bounds.left, p.bounds.top))
}

#[test]
fn the_planar_arrangement_sees_the_ellipse_not_its_box() {
    let (model, a, b) = model_with_two_ovals();
    let ids = [ElementId::Oval(a.clone()), ElementId::Oval(b)];
    let table = model.path_anchors(&ids[0]).expect("pathAnchors");
    // The raw-space point just inside the first oval's box corner: inside
    // the box, outside the ellipse.
    let xs: Vec<f32> = table.anchors.iter().map(|p| p.anchor[0]).collect();
    let ys: Vec<f32> = table.anchors.iter().map(|p| p.anchor[1]).collect();
    let left = xs.iter().cloned().fold(f32::INFINITY, f32::min);
    let top = ys.iter().cloned().fold(f32::INFINITY, f32::min);
    let corner = [left + 4.0, top + 4.0];
    let centre = [left + 50.0, top + 100.0];

    let at_corner = model.planar_regions(&ids, Some(corner));
    assert!(at_corner.found, "{:?}", at_corner.reason);
    assert!(
        at_corner.faces.is_empty(),
        "a box corner is outside the ellipse, yet a face answered: {:?}",
        at_corner.faces.iter().map(|f| &f.id).collect::<Vec<_>>()
    );
    let inside = model.planar_regions(&ids, Some(centre));
    assert!(inside.found, "{:?}", inside.reason);
    assert_eq!(inside.faces.len(), 1, "the ellipse's own face answers");

    // The whole arrangement: two overlapping discs make three faces, and
    // none of them is a rectangle (every face carries a curved segment).
    let all = model.planar_regions(&ids, None);
    assert!(all.found, "{:?}", all.reason);
    assert_eq!(all.faces.len(), 3, "left lune, lens, right lune");
    for face in &all.faces {
        assert!(
            face.anchors
                .iter()
                .any(|p| p.left != p.anchor || p.right != p.anchor),
            "face {} has only straight edges",
            face.id
        );
    }
}
