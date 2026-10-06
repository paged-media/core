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

//! Roadmap M1.3 — property tests for the Pathfinder boolean kernel.
//!
//! A seeded generator (no extra dependency, so the run is identical on
//! every machine) draws pairs of straight-edged shapes — axis-aligned
//! rectangles and convex polygons, overlapping, nested, touching or
//! apart — and checks the set algebra the four Shape Modes must obey:
//!
//!   |A ∪ B| + |A ∩ B| = |A| + |B|        (inclusion–exclusion)
//!   |A \ B| = |A| − |A ∩ B|
//!   |A ⊕ B| = |A ∪ B| − |A ∩ B|
//!   max(|A|,|B|) ≤ |A ∪ B|,  |A ∩ B| ≤ min(|A|,|B|)
//!
//! Areas come from the result's contours' SIGNED areas summed, which is
//! exact for straight edges and correct with holes because every
//! Pathfinder result runs in one stated direction with its holes the
//! other way (C-81). Inputs and results sit on the kernel's 1/64 pt grid
//! (C-21), so the tolerance is grid-sized, not a fudge: a few square
//! points of perimeter × 1/64.
//!
//! The first job of this file is that NOTHING panics: a panic in this
//! kernel is an abort in the wasm worker (C-21). `PAGED_BOOL_CASES` raises
//! the case count for a longer local or nightly run.

use paged_model::PathAnchor;
use paged_mutate::pathfinder::pathfinder_boolean;
use paged_mutate::pathfinder::PathfinderKind;

/// Park–Miller-style LCG: deterministic, seedable, no dependency.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    /// Uniform in [lo, hi), snapped to the kernel's 1/64 pt grid so the
    /// input itself carries no off-grid error.
    fn coord(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.next() % 1_000_000) as f32 / 1_000_000.0;
        ((lo + u * (hi - lo)) * 64.0).round() / 64.0
    }
}

type Shape = (Vec<PathAnchor>, Vec<usize>);

fn corner(x: f32, y: f32) -> PathAnchor {
    PathAnchor {
        anchor: (x, y),
        left: (x, y),
        right: (x, y),
    }
}

fn rect(r: &mut Lcg) -> Shape {
    let x0 = r.coord(0.0, 300.0);
    let y0 = r.coord(0.0, 300.0);
    let w = r.coord(8.0, 200.0);
    let h = r.coord(8.0, 200.0);
    (
        vec![
            corner(x0, y0),
            corner(x0 + w, y0),
            corner(x0 + w, y0 + h),
            corner(x0, y0 + h),
        ],
        vec![0],
    )
}

/// A convex polygon: n points on an ellipse at sorted angles.
fn convex(r: &mut Lcg) -> Shape {
    let cx = r.coord(60.0, 340.0);
    let cy = r.coord(60.0, 340.0);
    let rx = r.coord(20.0, 150.0);
    let ry = r.coord(20.0, 150.0);
    let n = 3 + (r.next() % 6) as usize;
    let mut angles: Vec<f32> = (0..n)
        .map(|_| (r.next() % 10_000) as f32 / 10_000.0 * std::f32::consts::TAU)
        .collect();
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    angles.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    if angles.len() < 3 {
        return rect(r);
    }
    let pts: Vec<PathAnchor> = angles
        .iter()
        .map(|t| {
            corner(
                ((cx + rx * t.cos()) * 64.0).round() / 64.0,
                ((cy + ry * t.sin()) * 64.0).round() / 64.0,
            )
        })
        .collect();
    (pts, vec![0])
}

fn shape(r: &mut Lcg) -> Shape {
    if r.next() % 2 == 0 {
        rect(r)
    } else {
        convex(r)
    }
}

/// Net area of a straight-edged result: the sum of its contours' signed
/// areas, sign-normalised to positive.
fn area((anchors, starts): &Shape) -> f64 {
    let mut total = 0.0f64;
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).copied().unwrap_or(anchors.len());
        let pts = &anchors[s..e];
        let mut a = 0.0f64;
        for k in 0..pts.len() {
            let (x0, y0) = pts[k].anchor;
            let (x1, y1) = pts[(k + 1) % pts.len()].anchor;
            a += x0 as f64 * y1 as f64 - x1 as f64 * y0 as f64;
        }
        total += a / 2.0;
    }
    total.abs()
}

fn perimeter((anchors, _): &Shape) -> f64 {
    let n = anchors.len();
    (0..n)
        .map(|k| {
            let (x0, y0) = anchors[k].anchor;
            let (x1, y1) = anchors[(k + 1) % n].anchor;
            ((x1 - x0) as f64).hypot((y1 - y0) as f64)
        })
        .sum()
}

fn op(a: &Shape, b: &Shape, kind: PathfinderKind) -> Shape {
    pathfinder_boolean(&[a.clone(), b.clone()], kind)
}

fn cases() -> usize {
    std::env::var("PAGED_BOOL_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

#[test]
fn the_four_shape_modes_obey_the_set_algebra() {
    let mut r = Lcg(20_261_004);
    let mut checked = 0;
    for case in 0..cases() {
        let a = shape(&mut r);
        let b = shape(&mut r);
        let (aa, ab) = (area(&a), area(&b));
        let union = area(&op(&a, &b, PathfinderKind::Union));
        let inter = area(&op(&a, &b, PathfinderKind::Intersect));
        let sub = area(&op(&a, &b, PathfinderKind::Subtract));
        let excl = area(&op(&a, &b, PathfinderKind::Exclude));
        // Grid error: every vertex can move by ≤ 1/128 pt per axis.
        let tol = (perimeter(&a) + perimeter(&b)) / 64.0 + 0.5;
        let ctx = || format!("case {case}: A={a:?} B={b:?}");
        assert!(
            (union + inter - (aa + ab)).abs() <= tol,
            "inclusion–exclusion: |A∪B|={union} |A∩B|={inter} |A|={aa} |B|={ab} tol={tol}; {}",
            ctx()
        );
        assert!(union + tol >= aa.max(ab), "|A∪B|={union} < max; {}", ctx());
        assert!(inter <= aa.min(ab) + tol, "|A∩B|={inter} > min; {}", ctx());
        assert!(
            (sub - (aa - inter)).abs() <= tol,
            "|A\\B|={sub} vs |A|-|A∩B|={}; {}",
            aa - inter,
            ctx()
        );
        assert!(
            (excl - (union - inter)).abs() <= tol,
            "|A⊕B|={excl} vs |A∪B|-|A∩B|={}; {}",
            union - inter,
            ctx()
        );
        checked += 1;
    }
    assert_eq!(checked, cases());
}

#[test]
fn a_shape_with_itself_is_itself_and_minus_itself_is_nothing() {
    let mut r = Lcg(7);
    for case in 0..cases() / 3 {
        let a = shape(&mut r);
        let aa = area(&a);
        let tol = perimeter(&a) / 64.0 + 0.5;
        for kind in [PathfinderKind::Union, PathfinderKind::Intersect] {
            let got = area(&op(&a, &a, kind));
            assert!(
                (got - aa).abs() <= tol,
                "case {case} {kind:?}: {got} vs {aa}; A={a:?}"
            );
        }
        assert!(
            area(&op(&a, &a, PathfinderKind::Subtract)) <= tol,
            "case {case}: A\\A not empty; A={a:?}"
        );
    }
}
