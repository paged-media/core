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

//! kurbo-backed geometry kernel (B-05, decision 5 of 2026-06-06).
//!
//! kurbo is Vello's own geometry vocabulary, so this is a promotion
//! into `paged-mutate`, not a new tree entry. It supplies what
//! `flo_curves` (booleans + Schneider fitting) does not:
//!
//!   - [`outline_stroke`] — stroke expansion (stroked path → filled
//!     outline), the §13.3 Tier-A op and the bake path for
//!     variable-width strokes later.
//!   - [`simplify_path`] — anchor reduction at a max-deviation
//!     tolerance (kurbo's `simplify_bezpath`).
//!   - [`offset_closed_path`] — parametric inset/outset for CLOSED
//!     contours: stroke the boundary at `2·|delta|`, then
//!     union (outset) / subtract (inset) against the original via
//!     the existing flo_curves boolean kernel. Robust where naive
//!     per-segment offsetting self-intersects; open-path offset is
//!     deliberately deferred (engine validates closed-only).
//!   - [`nearest_point_on_path`] — closest point on the path's
//!     cubics (B-06): collapses the third TS copy of `closestTOnCubic`
//!     once exposed as a worker query.
//!
//! All functions speak the document's anchor-table dialect
//! (`PathAnchor` + `subpath_starts` + `subpath_open`) and return the
//! same, so apply arms stay thin. Math runs in f64 (kurbo) and
//! converts back to the model's f32 at the boundary.

use kurbo::{
    simplify::{simplify_bezpath, SimplifyOptions},
    BezPath, CubicBez, ParamCurve, ParamCurveNearest, PathEl, PathSeg, Point,
    Stroke as KurboStroke, StrokeOpts,
};
use paged_parse::PathAnchor;


/// Stroke joins the wire/ops layer can request. Mirrors IDML's
/// stroke-join vocabulary (miter/round/bevel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeJoin {
    Miter,
    Round,
    Bevel,
}

/// Stroke caps for open contours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeCap {
    Butt,
    Round,
    Square,
}

/// Result of [`nearest_point_on_path`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearestPoint {
    /// Flat index of the segment's START anchor.
    pub seg_start: usize,
    /// Flat index of the segment's END anchor (wraps to the subpath
    /// start on a closing segment).
    pub seg_end: usize,
    /// Curve parameter on that segment, 0..=1.
    pub t: f32,
    /// The on-curve point.
    pub point: (f32, f32),
    /// Euclidean distance from the query point.
    pub distance: f32,
}

const EPS: f64 = 1e-6;
/// Flattening/expansion tolerance, pt. Stroke expansion and simplify
/// both take a max-deviation accuracy; 0.05 pt is far below visual
/// threshold at print sizes while keeping output anchor counts sane.
const TOLERANCE: f64 = 0.05;

fn pt(p: (f32, f32)) -> Point {
    Point::new(f64::from(p.0), f64::from(p.1))
}

fn xy(p: Point) -> (f32, f32) {
    (p.x as f32, p.y as f32)
}

/// Iterate `(start, end)` flat-index pairs per subpath, including
/// the wraparound closing pair for closed subpaths. Mirrors the
/// segment enumeration the path-edit overlay and the draw planner
/// use.
fn segment_pairs(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
) -> Vec<(usize, usize)> {
    let n = anchors.len();
    let starts: Vec<usize> = if subpath_starts.is_empty() {
        vec![0]
    } else {
        subpath_starts.to_vec()
    };
    let mut out = Vec::new();
    for (si, &sub_start) in starts.iter().enumerate() {
        let sub_end = starts.get(si + 1).copied().unwrap_or(n);
        for i in sub_start..sub_end.saturating_sub(1) {
            out.push((i, i + 1));
        }
        let open = subpath_open.get(si).copied().unwrap_or(false);
        if !open && sub_end - sub_start >= 2 {
            out.push((sub_end - 1, sub_start));
        }
    }
    out
}

fn cubic_for(a: &PathAnchor, b: &PathAnchor) -> CubicBez {
    CubicBez::new(pt(a.anchor), pt(a.right), pt(b.left), pt(b.anchor))
}

/// Anchor table → kurbo `BezPath`. Collapsed handles emit `LineTo`
/// so downstream algorithms see true lines, not degenerate cubics.
pub fn anchors_to_bezpath(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
) -> BezPath {
    let n = anchors.len();
    let starts: Vec<usize> = if subpath_starts.is_empty() {
        vec![0]
    } else {
        subpath_starts.to_vec()
    };
    let mut path = BezPath::new();
    for (si, &sub_start) in starts.iter().enumerate() {
        let sub_end = starts.get(si + 1).copied().unwrap_or(n);
        if sub_end <= sub_start {
            continue;
        }
        let open = subpath_open.get(si).copied().unwrap_or(false);
        path.move_to(pt(anchors[sub_start].anchor));
        let emit = |path: &mut BezPath, a: &PathAnchor, b: &PathAnchor| {
            let straight = (pt(a.right) - pt(a.anchor)).hypot() < EPS
                && (pt(b.left) - pt(b.anchor)).hypot() < EPS;
            if straight {
                path.line_to(pt(b.anchor));
            } else {
                path.curve_to(pt(a.right), pt(b.left), pt(b.anchor));
            }
        };
        for i in sub_start..sub_end - 1 {
            emit(&mut path, &anchors[i], &anchors[i + 1]);
        }
        if !open && sub_end - sub_start >= 2 {
            emit(&mut path, &anchors[sub_end - 1], &anchors[sub_start]);
            path.close_path();
        }
    }
    path
}

/// kurbo `BezPath` → anchor table. Quads elevate to cubics; a closing
/// segment whose endpoint coincides with the subpath start folds its
/// incoming handle into the start anchor's `left` (the IDML seam
/// convention the parser uses for closed contours).
pub fn bezpath_to_anchors(path: &BezPath) -> (Vec<PathAnchor>, Vec<usize>, Vec<bool>) {
    let mut anchors: Vec<PathAnchor> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut open_flags: Vec<bool> = Vec::new();
    let mut sub_start: Option<usize> = None;

    let corner = |p: Point| PathAnchor {
        anchor: xy(p),
        left: xy(p),
        right: xy(p),
    };

    let mut finish_subpath =
        |closed: bool, sub_start: &mut Option<usize>, anchors: &mut Vec<PathAnchor>| {
            if let Some(s) = sub_start.take() {
                if closed && anchors.len() > s + 1 {
                    // Fold the duplicated seam point: stroke/simplify
                    // output traces back to the start before ClosePath.
                    let first = anchors[s].anchor;
                    let last = anchors.last().expect("non-empty subpath");
                    let coincide = (pt(last.anchor) - pt(first)).hypot() < 1e-3;
                    if coincide {
                        let left = last.left;
                        anchors[s].left = left;
                        anchors.pop();
                    }
                }
                open_flags.push(!closed);
            }
        };

    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                finish_subpath(false, &mut sub_start, &mut anchors);
                starts.push(anchors.len());
                sub_start = Some(anchors.len());
                anchors.push(corner(p));
            }
            PathEl::LineTo(p) => {
                anchors.push(corner(p));
            }
            PathEl::QuadTo(c, p) => {
                // Elevate to cubic: controls at 2/3 along each quad
                // hull edge (exact representation of the quadratic).
                let prev = anchors.last_mut().expect("QuadTo after MoveTo");
                let p0 = pt(prev.anchor);
                let c1 = p0 + (c - p0) * (2.0 / 3.0);
                let c2 = p + (c - p) * (2.0 / 3.0);
                prev.right = xy(c1);
                let mut a = corner(p);
                a.left = xy(c2);
                anchors.push(a);
            }
            PathEl::CurveTo(c1, c2, p) => {
                let prev = anchors.last_mut().expect("CurveTo after MoveTo");
                prev.right = xy(c1);
                let mut a = corner(p);
                a.left = xy(c2);
                anchors.push(a);
            }
            PathEl::ClosePath => {
                finish_subpath(true, &mut sub_start, &mut anchors);
            }
        }
    }
    finish_subpath(false, &mut sub_start, &mut anchors);
    (anchors, starts, open_flags)
}


fn kurbo_join(j: StrokeJoin) -> kurbo::Join {
    match j {
        StrokeJoin::Miter => kurbo::Join::Miter,
        StrokeJoin::Round => kurbo::Join::Round,
        StrokeJoin::Bevel => kurbo::Join::Bevel,
    }
}

fn kurbo_cap(c: StrokeCap) -> kurbo::Cap {
    match c {
        StrokeCap::Butt => kurbo::Cap::Butt,
        StrokeCap::Round => kurbo::Cap::Round,
        StrokeCap::Square => kurbo::Cap::Square,
    }
}

/// Outline Stroke (§13.3): expand a stroked path into the filled
/// outline shape. Output contours are CLOSED by construction.
pub fn outline_stroke(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
    width: f32,
    cap: StrokeCap,
    join: StrokeJoin,
    miter_limit: f32,
) -> Option<(Vec<PathAnchor>, Vec<usize>, Vec<bool>)> {
    if anchors.is_empty() || width <= 0.0 {
        return None;
    }
    let path = anchors_to_bezpath(anchors, subpath_starts, subpath_open);
    let mut style = KurboStroke::new(f64::from(width));
    style.join = kurbo_join(join);
    style.miter_limit = f64::from(miter_limit.max(1.0));
    style.start_cap = kurbo_cap(cap);
    style.end_cap = kurbo_cap(cap);
    let outline = kurbo::stroke(path, &style, &StrokeOpts::default(), TOLERANCE);
    // RAW expansion output. kurbo draws conservative inner joins
    // that loop through source vertices — correct under the engine's
    // nonzero fill (paged-gpu fills FillRule::Winding), so we keep
    // the anchors as emitted rather than running a winding-sensitive
    // resolve. (Even-odd consumers would need a cleanup pass.)
    let (a, s, _o) = bezpath_to_anchors(&outline);
    if a.len() < 2 {
        return None;
    }
    let closed = vec![false; s.len().max(1)];
    Some((a, s, closed))
}

/// Simplify (§13.1): re-express the path within `tolerance` pt of the
/// original with (typically far) fewer anchors.
pub fn simplify_path(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
    tolerance: f32,
) -> Option<(Vec<PathAnchor>, Vec<usize>, Vec<bool>)> {
    if anchors.len() < 3 || tolerance <= 0.0 {
        return None;
    }
    let path = anchors_to_bezpath(anchors, subpath_starts, subpath_open);
    let simplified = simplify_bezpath(
        path,
        f64::from(tolerance),
        &SimplifyOptions::default(),
    );
    let (a, s, o) = bezpath_to_anchors(&simplified);
    if a.len() < 2 {
        return None;
    }
    Some((a, s, o))
}

/// Offset Path (§13.3) for a SINGLE closed contour. `delta > 0`
/// outsets, `delta < 0` insets.
///
/// Construction: EXACT per-segment parallel curves (flo's
/// least-mean-squares `offset`), consecutive offset runs joined with
/// straight connectors, then one nonzero-winding resolve
/// (`path_remove_interior_points`) to trim the crossings that
/// connectors create at corners. Because the parser's contour
/// winding is not normalized, BOTH ±delta candidates are built and
/// selected by enclosed area: the inset is the candidate smaller
/// than the original, the outset the larger one. An inset past the
/// medial axis resolves to nothing → None. Corner style is
/// miter-like on the trimmed side and bevel on the gap side (round/
/// miter joins are a follow-up); multi-subpath (holes) and open
/// inputs are deferred in v1.
pub fn offset_closed_path(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
    delta: f32,
    _join: StrokeJoin,
    _miter_limit: f32,
) -> Option<(Vec<PathAnchor>, Vec<usize>, Vec<bool>)> {
    use flo_curves::bezier::path::{path_remove_interior_points, SimpleBezierPath};
    use flo_curves::bezier::{offset, BezierCurveFactory, Curve};
    use flo_curves::geo::Coord2;
    use kurbo::Shape;

    if anchors.len() < 3 || delta == 0.0 {
        return None;
    }
    let single_closed =
        subpath_starts.len() <= 1 && !subpath_open.iter().any(|open| *open);
    if !single_closed {
        return None; // single closed contour only, v1 by design
    }

    let c2 = |p: (f32, f32)| Coord2(f64::from(p.0), f64::from(p.1));
    let original_area = anchors_to_bezpath(anchors, subpath_starts, subpath_open)
        .area()
        .abs();

    // Build one signed candidate: offset every segment, connect the
    // runs, resolve crossings, return the largest resolved contour
    // with its area.
    let candidate = |d: f64| -> Option<(Vec<PathAnchor>, f64)> {
        let mut segs: Vec<(Coord2, Coord2, Coord2)> = Vec::new();
        let mut start: Option<Coord2> = None;
        let mut prev_end: Option<Coord2> = None;
        for (s, e) in segment_pairs(anchors, subpath_starts, subpath_open) {
            let a = &anchors[s];
            let b = &anchors[e];
            let curve = Curve::from_points(
                c2(a.anchor),
                (c2(a.right), c2(b.left)),
                c2(b.anchor),
            );
            let pieces = offset(&curve, d, d);
            if pieces.is_empty() {
                return None;
            }
            for (pi, piece) in pieces.iter().enumerate() {
                let (ps, (cp1, cp2), pe) = (
                    piece.start_point,
                    piece.control_points,
                    piece.end_point,
                );
                if pi == 0 {
                    match (start, prev_end) {
                        (None, _) => start = Some(ps),
                        (Some(_), Some(prev)) => {
                            // Straight connector across the corner gap.
                            let l1 = Coord2(
                                prev.0 + (ps.0 - prev.0) / 3.0,
                                prev.1 + (ps.1 - prev.1) / 3.0,
                            );
                            let l2 = Coord2(
                                prev.0 + (ps.0 - prev.0) * 2.0 / 3.0,
                                prev.1 + (ps.1 - prev.1) * 2.0 / 3.0,
                            );
                            segs.push((l1, l2, ps));
                        }
                        _ => {}
                    }
                }
                segs.push((cp1, cp2, pe));
                prev_end = Some(pe);
            }
        }
        let start = start?;
        let prev = prev_end?;
        // Closing connector back to the start.
        let l1 = Coord2(
            prev.0 + (start.0 - prev.0) / 3.0,
            prev.1 + (start.1 - prev.1) / 3.0,
        );
        let l2 = Coord2(
            prev.0 + (start.0 - prev.0) * 2.0 / 3.0,
            prev.1 + (start.1 - prev.1) * 2.0 / 3.0,
        );
        segs.push((l1, l2, start));
        let raw: SimpleBezierPath = (start, segs);
        let resolved: Vec<SimpleBezierPath> =
            path_remove_interior_points(&vec![raw], 0.01);
        if resolved.is_empty() {
            return None;
        }
        let (ra, rs) = crate::bezier_conv::flo_to_idml_path(&resolved);
        // Largest resolved contour is the candidate boundary.
        let n = ra.len();
        let mut best: Option<(f64, usize, usize)> = None;
        for (si, &cs) in rs.iter().enumerate() {
            let ce = rs.get(si + 1).copied().unwrap_or(n);
            if ce - cs < 3 {
                continue;
            }
            let area = anchors_to_bezpath(&ra[cs..ce], &[0], &[false])
                .area()
                .abs();
            if best.map(|(ba, ..)| area > ba).unwrap_or(true) {
                best = Some((area, cs, ce));
            }
        }
        let (area, cs, ce) = best?;
        Some((ra[cs..ce].to_vec(), area))
    };

    let d = f64::from(delta.abs());
    let picked = match (candidate(d), candidate(-d)) {
        (a, b) => {
            let mut options: Vec<(Vec<PathAnchor>, f64)> =
                [a, b].into_iter().flatten().collect();
            if delta > 0.0 {
                options.retain(|(_, area)| *area > original_area + 1e-3);
                options.sort_by(|x, y| x.1.total_cmp(&y.1));
                options.pop()
            } else {
                options.retain(|(_, area)| {
                    *area > 1e-3 && *area < original_area - 1e-3
                });
                options.sort_by(|x, y| x.1.total_cmp(&y.1));
                options.into_iter().next()
            }
        }
    }?;
    if picked.0.len() < 3 {
        return None;
    }
    if delta < 0.0 {
        // Reject inverted artifacts an over-deep inset can resolve
        // into: every point of a TRUE depth-d inset keeps distance
        // ~d from the original boundary (0.9·d tolerance absorbs the
        // LMS fitting error).
        let min_clearance = picked
            .0
            .iter()
            .filter_map(|a| {
                nearest_point_on_path(anchors, subpath_starts, subpath_open, a.anchor)
            })
            .map(|hit| hit.distance)
            .fold(f32::MAX, f32::min);
        if min_clearance < delta.abs() * 0.9 {
            return None;
        }
    }
    Some((picked.0, vec![0], vec![false]))
}

/// Nearest on-curve point (B-06): the engine-side answer to the
/// `closestTOnCubic` math currently triplicated in TS.
pub fn nearest_point_on_path(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
    query: (f32, f32),
) -> Option<NearestPoint> {
    let q = pt(query);
    let mut best: Option<NearestPoint> = None;
    for (s, e) in segment_pairs(anchors, subpath_starts, subpath_open) {
        let seg = cubic_for(&anchors[s], &anchors[e]);
        let nearest = PathSeg::Cubic(seg).nearest(q, 1e-4);
        let p = seg.eval(nearest.t);
        let d = (p - q).hypot();
        if best.map(|b| d < f64::from(b.distance)).unwrap_or(true) {
            best = Some(NearestPoint {
                seg_start: s,
                seg_end: e,
                t: nearest.t as f32,
                point: xy(p),
                distance: d as f32,
            });
        }
    }
    best
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

    /// Closed unit-square fixture, one subpath.
    fn square() -> (Vec<PathAnchor>, Vec<usize>, Vec<bool>) {
        (
            vec![
                corner(0.0, 0.0),
                corner(100.0, 0.0),
                corner(100.0, 100.0),
                corner(0.0, 100.0),
            ],
            vec![0],
            vec![false],
        )
    }

    fn bbox(anchors: &[PathAnchor]) -> (f32, f32, f32, f32) {
        let xs = anchors.iter().map(|a| a.anchor.0);
        let ys = anchors.iter().map(|a| a.anchor.1);
        (
            xs.clone().fold(f32::MAX, f32::min),
            ys.clone().fold(f32::MAX, f32::min),
            xs.fold(f32::MIN, f32::max),
            ys.fold(f32::MIN, f32::max),
        )
    }

    #[test]
    fn anchors_bezpath_round_trip_closed() {
        let (a, s, o) = square();
        let path = anchors_to_bezpath(&a, &s, &o);
        let (a2, s2, o2) = bezpath_to_anchors(&path);
        assert_eq!(a2.len(), 4, "seam fold keeps 4 anchors");
        assert_eq!(s2, vec![0]);
        assert_eq!(o2, vec![false]);
        for (x, y) in a.iter().zip(a2.iter()) {
            assert!((pt(x.anchor) - pt(y.anchor)).hypot() < 1e-3);
        }
    }

    #[test]
    fn outline_stroke_of_a_line_is_a_closed_band() {
        let anchors = vec![corner(0.0, 0.0), corner(100.0, 0.0)];
        let (a, _s, o) = outline_stroke(
            &anchors,
            &[0],
            &[true],
            10.0,
            StrokeCap::Butt,
            StrokeJoin::Miter,
            4.0,
        )
        .expect("outline");
        assert!(o.iter().all(|open| !open), "outline contours are closed");
        let (x0, y0, x1, y1) = bbox(&a);
        // A 10pt butt-capped stroke of a 100pt horizontal line is a
        // 100 × 10 band centred on the line.
        assert!((x0 - 0.0).abs() < 0.5 && (x1 - 100.0).abs() < 0.5);
        assert!((y0 + 5.0).abs() < 0.5 && (y1 - 5.0).abs() < 0.5);
    }

    #[test]
    fn simplify_collapses_redundant_collinear_anchors() {
        // 11 anchors along one straight line → simplify to its ends.
        let anchors: Vec<PathAnchor> =
            (0..=10).map(|i| corner(i as f32 * 10.0, 0.0)).collect();
        let (a, _s, o) =
            simplify_path(&anchors, &[0], &[true], 0.25).expect("simplify");
        assert!(o[0], "stays open");
        assert!(
            a.len() <= 3,
            "collinear run should collapse, got {} anchors",
            a.len()
        );
        assert!((pt(a[0].anchor) - Point::new(0.0, 0.0)).hypot() < 1e-3);
        assert!(
            (pt(a.last().unwrap().anchor) - Point::new(100.0, 0.0)).hypot() < 1e-3
        );
    }

    #[test]
    fn offset_outset_and_inset_move_the_bbox_by_delta() {
        let (a, s, o) = square();
        let (oa, ..) =
            offset_closed_path(&a, &s, &o, 10.0, StrokeJoin::Miter, 4.0)
                .expect("outset");
        let (x0, y0, x1, y1) = bbox(&oa);
        assert!((x0 + 10.0).abs() < 0.5 && (y0 + 10.0).abs() < 0.5);
        assert!((x1 - 110.0).abs() < 0.5 && (y1 - 110.0).abs() < 0.5);

        let (ia, ..) =
            offset_closed_path(&a, &s, &o, -10.0, StrokeJoin::Miter, 4.0)
                .expect("inset");
        let (x0, y0, x1, y1) = bbox(&ia);
        assert!((x0 - 10.0).abs() < 0.5 && (y0 - 10.0).abs() < 0.5);
        assert!((x1 - 90.0).abs() < 0.5 && (y1 - 90.0).abs() < 0.5);
    }

    #[test]
    fn offset_rejects_open_paths_and_total_inset() {
        let open = (vec![corner(0.0, 0.0), corner(100.0, 0.0)], vec![0], vec![true]);
        assert!(offset_closed_path(&open.0, &open.1, &open.2, 5.0, StrokeJoin::Miter, 4.0)
            .is_none());
        let (a, s, o) = square();
        assert!(
            offset_closed_path(&a, &s, &o, -60.0, StrokeJoin::Miter, 4.0).is_none(),
            "inset past the medial axis consumes the shape"
        );
    }

    #[test]
    fn nearest_point_projects_onto_the_curve() {
        let (a, s, o) = square();
        // Query right of the right edge midpoint.
        let hit = nearest_point_on_path(&a, &s, &o, (130.0, 50.0)).expect("hit");
        assert_eq!((hit.seg_start, hit.seg_end), (1, 2), "right edge");
        assert!((hit.point.0 - 100.0).abs() < 1e-2);
        assert!((hit.point.1 - 50.0).abs() < 1e-2);
        assert!((hit.distance - 30.0).abs() < 1e-2);
        assert!((hit.t - 0.5).abs() < 1e-2);
    }
}

