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

//! C-81 — the direction a path kernel's output runs in.
//!
//! The kernels used to hand back whatever direction their construction
//! happened to produce, so the same operation's result ran one way or
//! the other depending on the shape, the sign of an offset, the
//! direction of the input and where it started. Anything downstream that
//! reads direction — a later boolean, a text-on-path, a stroke's dash
//! phase, an exporter — saw a different path for the same request.
//!
//! Illustrator normalises per operation (Illustrator 30.1.0, the
//! paged.draw oracle, every recorded case): offset path and outline
//! stroke come back CLOCKWISE, every Pathfinder result COUNTER-
//! CLOCKWISE, and a compound path's holes run against its outer
//! contour. The oracle states its directions in page space with y DOWN
//! — the engine's own — and they agree with the sign of the plain
//! signed area of the page-space coordinates: clockwise on the page is a
//! POSITIVE `Σ (x_i·y_{i+1} − x_{i+1}·y_i)`, counter-clockwise negative.
//! (Illustrator's own `area`, in its y-up document space, has the same
//! signs — its probe reports both.)
//!
//! So [`orient_contours`] takes the direction an operation's OUTER
//! contours must run in and turns every closed contour to it, holes the
//! other way. Outer-or-hole is decided by nesting depth (how many other
//! contours of the same path enclose it), so the alternation is what the
//! non-zero fill rule reads as the same region the result already
//! described: re-orienting changes the direction, never what is filled.
//! Open contours are left alone — their direction is the caller's
//! (an open path's start and end mean something).

use kurbo::{BezPath, ParamCurve, PathSeg, Point, Shape};
use paged_model::PathAnchor;

/// A direction as seen ON THE PAGE (y down).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// Clockwise on the page: a positive page-space signed area.
    Clockwise,
    /// Counter-clockwise on the page: a negative page-space signed area.
    CounterClockwise,
}

impl Turn {
    fn opposite(self) -> Self {
        match self {
            Turn::Clockwise => Turn::CounterClockwise,
            Turn::CounterClockwise => Turn::Clockwise,
        }
    }
}

fn pt(p: (f32, f32)) -> Point {
    Point::new(f64::from(p.0), f64::from(p.1))
}

/// One closed contour as a kurbo path (cubics between consecutive
/// anchors, wrapping around).
fn contour_path(run: &[PathAnchor]) -> BezPath {
    let mut path = BezPath::new();
    path.move_to(pt(run[0].anchor));
    for i in 0..run.len() {
        let (a, b) = (run[i], run[(i + 1) % run.len()]);
        path.curve_to(pt(a.right), pt(b.left), pt(b.anchor));
    }
    path.close_path();
    path
}

/// The page-space signed area of one closed contour: positive when it
/// runs clockwise on the page. Exact for the Beziers.
pub fn signed_area(run: &[PathAnchor]) -> f64 {
    if run.len() < 2 {
        return 0.0;
    }
    contour_path(run).area()
}

/// The direction a closed contour runs in on the page, `None` for a
/// degenerate one (no area to have a direction).
pub fn turn_of(run: &[PathAnchor]) -> Option<Turn> {
    let a = signed_area(run);
    if a > 1e-9 {
        Some(Turn::Clockwise)
    } else if a < -1e-9 {
        Some(Turn::CounterClockwise)
    } else {
        None
    }
}

/// Reverse a contour in place, keeping its first anchor first: the run
/// `a0, a1, …, an-1` becomes `a0, an-1, …, a1`, and every anchor's
/// handles swap (what led into a point now leads out of it).
fn reverse_run(run: &mut [PathAnchor]) {
    // (With two anchors the order is the same either way round; the
    // handle swap below is what reverses it.)
    if !run.is_empty() {
        run[1..].reverse();
    }
    for a in run.iter_mut() {
        std::mem::swap(&mut a.left, &mut a.right);
    }
}

/// A point ON a contour, for testing which other contours enclose it:
/// the middle of its first segment, which is never a vertex shared with
/// a neighbour the way an anchor can be.
fn probe_point(run: &[PathAnchor]) -> Point {
    let (a, b) = (run[0], run[1 % run.len()]);
    let seg = PathSeg::Cubic(kurbo::CubicBez::new(
        pt(a.anchor),
        pt(a.right),
        pt(b.left),
        pt(b.anchor),
    ));
    seg.eval(0.5)
}

/// Turn every closed contour of a path so the OUTER contours run
/// `outer` and every hole the other way (see the module doc). `starts`
/// and `open` are the path's subpath tables (empty `starts` = one
/// contour); they are not changed — a reversal keeps each contour's
/// first anchor where it was.
pub fn orient_contours(
    anchors: &mut [PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
    outer: Turn,
) {
    let starts: Vec<usize> = if subpath_starts.is_empty() {
        vec![0]
    } else {
        subpath_starts.to_vec()
    };
    let n = anchors.len();
    let spans: Vec<(usize, usize, bool)> = starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let e = starts.get(i + 1).copied().unwrap_or(n);
            (s, e, subpath_open.get(i).copied().unwrap_or(false))
        })
        .filter(|&(s, e, _)| s < e && e <= n)
        .collect();
    let closed: Vec<(usize, usize)> = spans
        .iter()
        .filter(|&&(s, e, open)| !open && e - s >= 2)
        .map(|&(s, e, _)| (s, e))
        .collect();
    let paths: Vec<BezPath> = closed
        .iter()
        .map(|&(s, e)| contour_path(&anchors[s..e]))
        .collect();
    let mut wanted: Vec<Turn> = Vec::with_capacity(closed.len());
    for (i, &(s, e)) in closed.iter().enumerate() {
        let probe = probe_point(&anchors[s..e]);
        let depth = paths
            .iter()
            .enumerate()
            .filter(|&(j, other)| j != i && other.winding(probe) != 0)
            .count();
        wanted.push(if depth % 2 == 0 {
            outer
        } else {
            outer.opposite()
        });
    }
    for (&(s, e), want) in closed.iter().zip(wanted) {
        if let Some(turn) = turn_of(&anchors[s..e]) {
            if turn != want {
                reverse_run(&mut anchors[s..e]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corner(x: f32, y: f32) -> PathAnchor {
        PathAnchor {
            anchor: (x, y),
            left: (x, y),
            right: (x, y),
        }
    }

    /// Top edge left-to-right, then down: clockwise on a y-down page.
    fn square_cw(l: f32, t: f32, r: f32, b: f32) -> Vec<PathAnchor> {
        vec![corner(l, t), corner(r, t), corner(r, b), corner(l, b)]
    }

    #[test]
    fn clockwise_on_the_page_is_a_positive_area() {
        let sq = square_cw(0.0, 0.0, 100.0, 100.0);
        assert!((signed_area(&sq) - 10_000.0).abs() < 1e-6);
        assert_eq!(turn_of(&sq), Some(Turn::Clockwise));
        let mut rev = sq.clone();
        reverse_run(&mut rev);
        assert_eq!(turn_of(&rev), Some(Turn::CounterClockwise));
        assert_eq!(rev[0].anchor, sq[0].anchor, "the first anchor stays first");
    }

    #[test]
    fn outer_contours_take_the_direction_and_holes_the_other() {
        // A frame (outer, written COUNTER-clockwise) with a hole
        // (written clockwise — the same direction as the outer would
        // have to be) and an island inside the hole.
        let mut outer = square_cw(0.0, 0.0, 300.0, 300.0);
        reverse_run(&mut outer);
        let hole = square_cw(50.0, 50.0, 250.0, 250.0);
        let island = square_cw(100.0, 100.0, 200.0, 200.0);
        let mut anchors: Vec<PathAnchor> = Vec::new();
        anchors.extend(&outer);
        anchors.extend(&hole);
        anchors.extend(&island);
        let starts = vec![0, 4, 8];
        let open = vec![false; 3];
        orient_contours(&mut anchors, &starts, &open, Turn::Clockwise);
        assert_eq!(turn_of(&anchors[0..4]), Some(Turn::Clockwise), "outer");
        assert_eq!(
            turn_of(&anchors[4..8]),
            Some(Turn::CounterClockwise),
            "hole"
        );
        assert_eq!(turn_of(&anchors[8..12]), Some(Turn::Clockwise), "island");
        orient_contours(&mut anchors, &starts, &open, Turn::CounterClockwise);
        assert_eq!(turn_of(&anchors[0..4]), Some(Turn::CounterClockwise));
        assert_eq!(turn_of(&anchors[4..8]), Some(Turn::Clockwise));
        assert_eq!(turn_of(&anchors[8..12]), Some(Turn::CounterClockwise));
    }

    #[test]
    fn an_open_contour_is_left_alone() {
        let mut anchors = vec![corner(0.0, 0.0), corner(100.0, 0.0), corner(100.0, 100.0)];
        let before = anchors.clone();
        orient_contours(&mut anchors, &[0], &[true], Turn::CounterClockwise);
        assert_eq!(
            anchors.iter().map(|a| a.anchor).collect::<Vec<_>>(),
            before.iter().map(|a| a.anchor).collect::<Vec<_>>()
        );
    }
}
