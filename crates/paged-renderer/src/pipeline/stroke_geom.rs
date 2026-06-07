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

//! Stroke-STYLE geometry helpers (W1.2).
//!
//! Pure functions that take a flattened polyline of a stroke path and
//! produce the offset / sampled polylines the striped + wavy stroke
//! styles need:
//!
//!   * [`flatten_path`] — tessellate a [`PathData`]'s cubics into a
//!     dense polyline (one polyline per contour). Curves are flattened
//!     to line segments for offsetting purposes — InDesign's striped /
//!     wavy strokes are decorative and a fine polyline is visually
//!     indistinguishable from the true offset Bezier at print
//!     resolution. Documented as a deliberate approximation.
//!   * [`offset_polyline`] — shift every vertex of a polyline along its
//!     local averaged-normal by a signed distance (perpendicular
//!     offset). Used for the striped sub-rules.
//!   * [`offset_closed_outline`] — W1.5 miter-correct inward / outward
//!     offset of a CLOSED contour for stroke alignment (Inside /
//!     Outside) on ovals, closed polygons, and closed compound paths.
//!     Picks the inset / outset candidate by enclosed area so it is
//!     winding-agnostic; open sub-contours stay centred.
//!   * [`sine_polyline`] — resample a polyline by arc length, displacing
//!     each sample along the local normal by `amplitude·sin(2π·s/period)`
//!     to make the wavy-stroke centreline.
//!   * [`polyline_to_path`] / [`polylines_to_path`] — wrap the results
//!     back into a [`PathData`] (open contours, no `Close`).
//!
//! All math is in the path's local (inner) coordinate space; the caller
//! applies the frame transform when emitting.

use paged_compose::{PathData, PathSegment};

/// A flattened contour: an ordered list of `(x, y)` vertices and a flag
/// recording whether the source contour was closed (so callers can
/// re-close after offsetting if they need an outline).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Polyline {
    pub points: Vec<(f32, f32)>,
    pub closed: bool,
}

/// Default cubic-flattening density. Eight segments per cubic keeps the
/// chord error well under a pixel for the curve radii InDesign uses on
/// decorative strokes, while staying cheap enough to run per frame.
pub(crate) const FLATTEN_STEPS: u32 = 8;

/// Tessellate a [`PathData`] into one [`Polyline`] per contour. Cubics
/// and quadratics are sampled at [`FLATTEN_STEPS`] points; lines pass
/// through unchanged. A `Close` marks the current contour closed and
/// terminates it. Empty / degenerate contours are dropped.
///
/// NOTE: this flattens curves to polylines. The striped + wavy stroke
/// styles only ever offset / resample this polyline, so the small chord
/// error is acceptable and intentional (see module docs).
pub(crate) fn flatten_path(path: &PathData, steps: u32) -> Vec<Polyline> {
    let n = steps.max(1);
    let mut out: Vec<Polyline> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut start = (0.0f32, 0.0f32);
    let mut pen = (0.0f32, 0.0f32);
    let flush = |cur: &mut Vec<(f32, f32)>, out: &mut Vec<Polyline>, closed: bool| {
        if cur.len() >= 2 {
            out.push(Polyline {
                points: std::mem::take(cur),
                closed,
            });
        } else {
            cur.clear();
        }
    };
    for seg in &path.segments {
        match *seg {
            PathSegment::MoveTo { x, y } => {
                flush(&mut cur, &mut out, false);
                start = (x, y);
                pen = (x, y);
                cur.push((x, y));
            }
            PathSegment::LineTo { x, y } => {
                cur.push((x, y));
                pen = (x, y);
            }
            PathSegment::QuadTo { cx, cy, x, y } => {
                let (p0x, p0y) = pen;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let mt = 1.0 - t;
                    let px = mt * mt * p0x + 2.0 * mt * t * cx + t * t * x;
                    let py = mt * mt * p0y + 2.0 * mt * t * cy + t * t * y;
                    cur.push((px, py));
                }
                pen = (x, y);
            }
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                let (p0x, p0y) = pen;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let mt = 1.0 - t;
                    let px = mt * mt * mt * p0x
                        + 3.0 * mt * mt * t * cx1
                        + 3.0 * mt * t * t * cx2
                        + t * t * t * x;
                    let py = mt * mt * mt * p0y
                        + 3.0 * mt * mt * t * cy1
                        + 3.0 * mt * t * t * cy2
                        + t * t * t * y;
                    cur.push((px, py));
                }
                pen = (x, y);
            }
            PathSegment::Close => {
                // Snap back to the contour start so the closed loop is
                // geometrically shut (the renderer's own `Close` does
                // the same), then flush as closed.
                if cur.last() != Some(&start) {
                    cur.push(start);
                }
                pen = start;
                flush(&mut cur, &mut out, true);
            }
        }
    }
    flush(&mut cur, &mut out, false);
    out
}

/// Per-vertex unit normals for a polyline. For an interior vertex the
/// normal is the average of the two adjacent edge normals (so corners
/// offset smoothly); endpoints use their single edge.
///
/// The normal is the edge direction `(dx, dy)` rotated to `(-dy, dx)`.
/// In IDML / screen space (y grows downward) this points to the
/// **right-hand** side when walking the polyline in vertex order. A
/// positive offset distance therefore moves to the right of travel;
/// negative moves left. The absolute handedness doesn't matter for the
/// callers (striped sub-rules use symmetric ± offsets; alignment picks
/// the sign that shrinks/grows the closed outline) — only that it is
/// consistent.
fn vertex_normals(points: &[(f32, f32)], closed: bool) -> Vec<(f32, f32)> {
    let m = points.len();
    if m < 2 {
        return vec![(0.0, 0.0); m];
    }
    // Edge unit normals: normal[i] belongs to the edge points[i]→[i+1].
    let edge_normal = |a: (f32, f32), b: (f32, f32)| -> (f32, f32) {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            (0.0, 0.0)
        } else {
            (-dy / len, dx / len)
        }
    };
    let mut normals = Vec::with_capacity(m);
    for i in 0..m {
        let prev = if i == 0 {
            if closed {
                Some(edge_normal(points[m - 1], points[0]))
            } else {
                None
            }
        } else {
            Some(edge_normal(points[i - 1], points[i]))
        };
        let next = if i + 1 < m {
            Some(edge_normal(points[i], points[i + 1]))
        } else if closed {
            Some(edge_normal(points[m - 1], points[0]))
        } else {
            None
        };
        let (nx, ny) = match (prev, next) {
            (Some(a), Some(b)) => (a.0 + b.0, a.1 + b.1),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => (0.0, 0.0),
        };
        let len = (nx * nx + ny * ny).sqrt();
        if len < 1e-6 {
            normals.push((0.0, 0.0));
        } else {
            normals.push((nx / len, ny / len));
        }
    }
    normals
}

/// Offset every vertex of `line` along its local normal by `distance`
/// (signed: positive moves to the right of travel — see
/// [`vertex_normals`]). Returns a polyline with the same vertex count
/// and `closed` flag.
pub(crate) fn offset_polyline(line: &Polyline, distance: f32) -> Polyline {
    let normals = vertex_normals(&line.points, line.closed);
    let points = line
        .points
        .iter()
        .zip(normals.iter())
        .map(|(&(x, y), &(nx, ny))| (x + nx * distance, y + ny * distance))
        .collect();
    Polyline {
        points,
        closed: line.closed,
    }
}

/// Drop vertices of a CLOSED polyline that are near-duplicates of their
/// predecessor or that lie (within a tolerance) on the straight line
/// between their neighbours. The result keeps only genuine corners +
/// curve samples, which is what the miter offset needs. Tolerance is in
/// the cross-product (≈ area) domain; `1e-2` keeps real curve detail
/// while collapsing the cubic-sampled straight edges of a polygon.
fn simplify_collinear(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let m = points.len();
    if m < 3 {
        return points.to_vec();
    }
    // First pass: drop consecutive near-duplicate vertices.
    let mut dedup: Vec<(f32, f32)> = Vec::with_capacity(m);
    for &p in points {
        // `map_or(true, …)` (not `is_none_or`, which is 1.82+; MSRV 1.80).
        if dedup
            .last()
            .map_or(true, |&q: &(f32, f32)| (p.0 - q.0).hypot(p.1 - q.1) > 1e-4)
        {
            dedup.push(p);
        }
    }
    if dedup.len() >= 2 && dedup.first() == dedup.last() {
        dedup.pop();
    }
    let n = dedup.len();
    if n < 3 {
        return dedup;
    }
    // Second pass: drop collinear interior vertices (treating the loop
    // as closed). A vertex is collinear when the triangle it forms with
    // its kept neighbours has near-zero area, normalised by the longer
    // adjacent edge so the test is scale-aware.
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(n);
    for i in 0..n {
        let prev = *out.last().unwrap_or(&dedup[(i + n - 1) % n]);
        let cur = dedup[i];
        let next = dedup[(i + 1) % n];
        let v1 = (cur.0 - prev.0, cur.1 - prev.1);
        let v2 = (next.0 - cur.0, next.1 - cur.1);
        let cross = v1.0 * v2.1 - v1.1 * v2.0;
        let scale = (v1.0.hypot(v1.1)).max(v2.0.hypot(v2.1)).max(1e-6);
        if (cross / scale).abs() > 1e-2 {
            out.push(cur);
        }
    }
    if out.len() < 3 {
        dedup
    } else {
        out
    }
}

/// W1.5 — miter-correct parallel offset of a closed polyline. Each
/// vertex moves along the **bisector** of its two adjacent edge
/// normals, scaled by `1/(n_bisector · n_edge)` so that BOTH adjacent
/// edges end up offset by exactly `distance` (a true parallel offset).
/// A plain averaged-unit-normal offset (`offset_polyline`) under-shoots
/// sharp corners — a 90° corner of a rect would inset by `distance/√2`
/// instead of `distance` — which is wrong for an alignment offset.
///
/// The miter scale blows up as the corner angle → 0; we clamp it to
/// `MITER_CLAMP` so an acute spike doesn't shoot the offset vertex off
/// to infinity (matching a stroke miter limit). The acute-spike
/// self-intersection that remains is the documented limitation on
/// [`offset_closed_outline`].
fn miter_offset_closed(points: &[(f32, f32)], distance: f32) -> Vec<(f32, f32)> {
    const MITER_CLAMP: f32 = 8.0;
    // Collapse near-duplicate and collinear runs first. A polygon's
    // straight edges arrive here as a dense cubic flattening (8 samples
    // per edge, clustered near corners by the control-point placement);
    // those clustered samples give the miter bisector a spurious
    // diagonal tilt that dents the offset outline inward. Reducing each
    // straight run back to its corner makes the corner miter exact and
    // leaves genuinely-curved outlines (ovals) almost untouched (their
    // samples are never collinear).
    let points = simplify_collinear(points);
    let m = points.len();
    if m < 3 {
        return points;
    }
    let edge_normal = |a: (f32, f32), b: (f32, f32)| -> Option<(f32, f32)> {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            None
        } else {
            Some((-dy / len, dx / len))
        }
    };
    let mut out = Vec::with_capacity(m);
    for i in 0..m {
        let prev_n = edge_normal(points[(i + m - 1) % m], points[i]);
        let next_n = edge_normal(points[i], points[(i + 1) % m]);
        let (bx, by, base) = match (prev_n, next_n) {
            (Some(a), Some(b)) => {
                let sx = a.0 + b.0;
                let sy = a.1 + b.1;
                let len = (sx * sx + sy * sy).sqrt();
                if len < 1e-6 {
                    // 180° reversal — keep the single edge normal.
                    (a.0, a.1, a)
                } else {
                    (sx / len, sy / len, a)
                }
            }
            (Some(a), None) | (None, Some(a)) => (a.0, a.1, a),
            (None, None) => (0.0, 0.0, (0.0, 0.0)),
        };
        // cos(half-angle) = bisector · edge_normal. Miter factor =
        // 1/cos, clamped so acute corners don't explode.
        let cos_half = (bx * base.0 + by * base.1).abs().max(1.0 / MITER_CLAMP);
        let scale = distance / cos_half;
        out.push((points[i].0 + bx * scale, points[i].1 + by * scale));
    }
    out
}

/// Signed area of a (closed) polyline via the shoelace formula. In
/// IDML / screen space (y grows downward) a clockwise contour yields a
/// **positive** area and a counter-clockwise one a negative area; only
/// the magnitude and the relative ordering of two candidates matter to
/// [`offset_closed_outline`], so the absolute sign convention is
/// irrelevant. The contour is treated as implicitly closed (the last
/// vertex connects back to the first).
fn polyline_signed_area(points: &[(f32, f32)]) -> f32 {
    let m = points.len();
    if m < 3 {
        return 0.0;
    }
    let mut acc = 0.0f32;
    for i in 0..m {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % m];
        acc += x0 * y1 - x1 * y0;
    }
    acc * 0.5
}

/// W1.5 — offset a CLOSED contour's outline inward or outward by
/// `distance` for stroke alignment (Inside / Outside) on non-rect
/// shapes (ovals, closed polygons, closed compound paths). `inward =
/// true` shrinks the outline (InsideAlignment), `false` grows it
/// (OutsideAlignment).
///
/// Strategy: flatten is already done by the caller; here we offset the
/// vertices along their averaged normals by `±distance` and pick the
/// sign by enclosed area — the inward result is the candidate with the
/// **smaller** absolute area, the outward result the **larger** one.
/// This is winding-agnostic (the IDML parser does not normalise contour
/// winding, so we cannot assume a fixed handedness for the per-vertex
/// normal) and mirrors `paged_mutate::kurbo_kernel::offset_closed_path`'s
/// area-selection trick.
///
/// LIMITATION: this is a per-vertex parallel offset, not a true
/// medial-axis offset. On a convex outline (oval, rounded polygon) it
/// is exact at print resolution. On a contour with acute interior
/// spikes a large inward offset can make the offset edges cross
/// (self-intersection) near the spike; we do not trim those crossings.
/// InDesign clamps such cases too, and the W1.* fixtures stay within
/// the convex / gently-concave regime where the artefact does not
/// appear. The proper offset (kurbo `offset_closed_path`, B-05) is NOT
/// reusable here: it lives in `paged-mutate`, which depends on
/// `paged-renderer` — wiring it back would form a dependency cycle.
pub(crate) fn offset_closed_outline(line: &Polyline, distance: f32, inward: bool) -> Polyline {
    if line.points.len() < 3 || distance.abs() < 1e-6 {
        return line.clone();
    }
    // `flatten_path` snaps a closed contour back to its start, leaving a
    // trailing vertex equal to the first. That duplicate gives the wrap
    // vertex a zero-length adjacent edge → a degenerate (0,0) normal →
    // it doesn't move under the offset, pinning one corner to the
    // unoffset outline. Drop it so the closed loop has distinct vertices.
    let line = if line.points.len() >= 2 && line.points.first() == line.points.last() {
        Polyline {
            points: line.points[..line.points.len() - 1].to_vec(),
            closed: true,
        }
    } else {
        line.clone()
    };
    if line.points.len() < 3 {
        return line;
    }
    let plus = Polyline {
        points: miter_offset_closed(&line.points, distance),
        closed: true,
    };
    let minus = Polyline {
        points: miter_offset_closed(&line.points, -distance),
        closed: true,
    };
    let area_plus = polyline_signed_area(&plus.points).abs();
    let area_minus = polyline_signed_area(&minus.points).abs();
    // Inward ⇒ smaller enclosed area; outward ⇒ larger.
    let pick_plus = if inward {
        area_plus <= area_minus
    } else {
        area_plus >= area_minus
    };
    if pick_plus {
        plus
    } else {
        minus
    }
}

/// Resample `line` along its arc length and displace each sample along
/// the local normal by `amplitude·sin(2π·s/period)`, producing a sine
/// wave that rides the path centreline. `period` and `amplitude` are in
/// pt; `samples_per_period` controls smoothness. Returns an open
/// polyline (a wavy stroke is always rendered as an open ribbon, even
/// over a closed source path, matching InDesign's preview).
pub(crate) fn sine_polyline(
    line: &Polyline,
    amplitude: f32,
    period: f32,
    samples_per_period: u32,
) -> Polyline {
    let pts = &line.points;
    if pts.len() < 2 || period <= 1e-3 {
        return Polyline {
            points: pts.clone(),
            closed: false,
        };
    }
    // Cumulative arc length per source vertex.
    let mut cum = Vec::with_capacity(pts.len());
    cum.push(0.0f32);
    let mut total = 0.0f32;
    for w in pts.windows(2) {
        let dx = w[1].0 - w[0].0;
        let dy = w[1].1 - w[0].1;
        total += (dx * dx + dy * dy).sqrt();
        cum.push(total);
    }
    if total <= 1e-3 {
        return Polyline {
            points: pts.clone(),
            closed: false,
        };
    }
    let per_period = samples_per_period.max(2);
    let n_samples = ((total / period) * per_period as f32).ceil().max(2.0) as usize;
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(n_samples + 1);
    for i in 0..=n_samples {
        let s = total * (i as f32 / n_samples as f32);
        // Locate the source segment containing arc length `s`.
        let seg = match cum.binary_search_by(|c| c.partial_cmp(&s).unwrap()) {
            Ok(idx) => idx.min(pts.len() - 2),
            Err(idx) => idx.saturating_sub(1).min(pts.len() - 2),
        };
        let seg_len = cum[seg + 1] - cum[seg];
        let local_t = if seg_len > 1e-6 {
            (s - cum[seg]) / seg_len
        } else {
            0.0
        };
        let (ax, ay) = pts[seg];
        let (bx, by) = pts[seg + 1];
        let cx = ax + (bx - ax) * local_t;
        let cy = ay + (by - ay) * local_t;
        // Local tangent → left normal.
        let dx = bx - ax;
        let dy = by - ay;
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if len > 1e-6 {
            (-dy / len, dx / len)
        } else {
            (0.0, 0.0)
        };
        let disp = amplitude * (std::f32::consts::TAU * s / period).sin();
        out.push((cx + nx * disp, cy + ny * disp));
    }
    Polyline {
        points: out,
        closed: false,
    }
}

/// Wrap a polyline into a [`PathData`] as a single open contour
/// (`MoveTo` + `LineTo`…, no `Close`). Closed input still emits an
/// explicit `Close` so a filled/stroked loop renders shut.
pub(crate) fn polyline_to_path(line: &Polyline) -> PathData {
    let mut segments = Vec::with_capacity(line.points.len() + 1);
    let mut it = line.points.iter();
    if let Some(&(x, y)) = it.next() {
        segments.push(PathSegment::MoveTo { x, y });
        for &(x, y) in it {
            segments.push(PathSegment::LineTo { x, y });
        }
        if line.closed {
            segments.push(PathSegment::Close);
        }
    }
    PathData { segments }
}

/// Wrap several polylines into one multi-contour [`PathData`]. Each
/// polyline becomes its own `MoveTo … (Close)` run, so a compound
/// outline (e.g. an offset polygon with a hole) round-trips through a
/// single path. Empty contours are skipped.
pub(crate) fn polylines_to_path(lines: &[Polyline]) -> PathData {
    let mut segments = Vec::new();
    for line in lines {
        let mut it = line.points.iter();
        if let Some(&(x, y)) = it.next() {
            segments.push(PathSegment::MoveTo { x, y });
            for &(x, y) in it {
                segments.push(PathSegment::LineTo { x, y });
            }
            if line.closed {
                segments.push(PathSegment::Close);
            }
        }
    }
    PathData { segments }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(points: &[(f32, f32)], closed: bool) -> Polyline {
        Polyline {
            points: points.to_vec(),
            closed,
        }
    }

    #[test]
    fn flatten_path_straight_line_passes_through() {
        let p = PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: 10.0, y: 0.0 },
            ],
        };
        let lines = flatten_path(&p, FLATTEN_STEPS);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].points, vec![(0.0, 0.0), (10.0, 0.0)]);
        assert!(!lines[0].closed);
    }

    #[test]
    fn flatten_path_cubic_produces_dense_samples_and_closes() {
        let p = PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::CubicTo {
                    cx1: 0.0,
                    cy1: 10.0,
                    cx2: 10.0,
                    cy2: 10.0,
                    x: 10.0,
                    y: 0.0,
                },
                PathSegment::Close,
            ],
        };
        let lines = flatten_path(&p, 8);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        // 1 move + 8 cubic samples + closing snap-back to start.
        assert_eq!(lines[0].points.len(), 10);
        assert_eq!(*lines[0].points.first().unwrap(), (0.0, 0.0));
        assert_eq!(*lines[0].points.last().unwrap(), (0.0, 0.0));
    }

    #[test]
    fn flatten_path_two_contours() {
        let p = PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: 1.0, y: 0.0 },
                PathSegment::Close,
                PathSegment::MoveTo { x: 5.0, y: 5.0 },
                PathSegment::LineTo { x: 6.0, y: 5.0 },
            ],
        };
        let lines = flatten_path(&p, FLATTEN_STEPS);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].closed);
        assert!(!lines[1].closed);
    }

    #[test]
    fn offset_horizontal_line_shifts_perpendicular() {
        // Walking +x, the normal (-dy, dx) points +y. A +2 offset moves
        // the line down by 2 (toward +y); a -2 offset moves it up by 2.
        let line = poly(&[(0.0, 0.0), (10.0, 0.0)], false);
        let down = offset_polyline(&line, 2.0);
        assert_eq!(down.points, vec![(0.0, 2.0), (10.0, 2.0)]);
        let up = offset_polyline(&line, -2.0);
        assert_eq!(up.points, vec![(0.0, -2.0), (10.0, -2.0)]);
    }

    #[test]
    fn offset_vertical_line_shifts_perpendicular() {
        // Walking +y, the normal (-dy, dx) points -x. A +3 offset moves
        // -x.
        let line = poly(&[(0.0, 0.0), (0.0, 10.0)], false);
        let off = offset_polyline(&line, 3.0);
        for (i, &(x, y)) in off.points.iter().enumerate() {
            assert!((x + 3.0).abs() < 1e-5, "pt {i} x={x}");
            assert!((y - (i as f32 * 10.0)).abs() < 1e-5, "pt {i} y={y}");
        }
    }

    #[test]
    fn offset_preserves_vertex_count_and_closed_flag() {
        let line = poly(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], true);
        let off = offset_polyline(&line, 1.5);
        assert_eq!(off.points.len(), line.points.len());
        assert!(off.closed);
    }

    #[test]
    fn offset_corner_uses_averaged_normal() {
        // An L-corner: walking +x then +y. The corner vertex's normal is
        // the bisector of the two edge normals. Edge1 (+x) normal = +y;
        // edge2 (+y) normal = -x; average = (-x, +y) normalised. With
        // distance √2 the corner (10,0) shifts to (9, 1).
        let line = poly(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], false);
        let off = offset_polyline(&line, (2.0f32).sqrt());
        let corner = off.points[1];
        assert!((corner.0 - 9.0).abs() < 1e-4, "corner x={}", corner.0);
        assert!((corner.1 - 1.0).abs() < 1e-4, "corner y={}", corner.1);
    }

    #[test]
    fn sine_polyline_zero_amplitude_tracks_centreline() {
        let line = poly(&[(0.0, 0.0), (20.0, 0.0)], false);
        let wave = sine_polyline(&line, 0.0, 4.0, 8);
        // Every sample sits on y=0 with x monotonically increasing.
        for w in wave.points.windows(2) {
            assert!(w[1].0 >= w[0].0);
            assert!(w[1].1.abs() < 1e-4);
        }
        assert!(!wave.closed);
    }

    #[test]
    fn sine_polyline_displaces_within_amplitude() {
        let line = poly(&[(0.0, 0.0), (40.0, 0.0)], false);
        let amp = 3.0;
        let wave = sine_polyline(&line, amp, 8.0, 8);
        let max_dev = wave
            .points
            .iter()
            .map(|&(_, y)| y.abs())
            .fold(0.0f32, f32::max);
        // Reaches close to the amplitude but never exceeds it.
        assert!(max_dev <= amp + 1e-4, "max_dev={max_dev}");
        assert!(max_dev > amp * 0.7, "wave too flat: max_dev={max_dev}");
    }

    #[test]
    fn sine_polyline_quarter_period_peaks_at_amplitude() {
        // A 40pt line with period 40 → one full wave. The sample at
        // s=10 (quarter period) hits +amplitude along the normal
        // (walking +x ⇒ normal +y), i.e. y ≈ +amp.
        let line = poly(&[(0.0, 0.0), (40.0, 0.0)], false);
        let wave = sine_polyline(&line, 5.0, 40.0, 40);
        let near = wave
            .points
            .iter()
            .min_by(|a, b| (a.0 - 10.0).abs().partial_cmp(&(b.0 - 10.0).abs()).unwrap())
            .unwrap();
        assert!((near.1 - 5.0).abs() < 0.2, "quarter-period y={}", near.1);
    }

    #[test]
    fn polyline_to_path_open_has_no_close() {
        let line = poly(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)], false);
        let p = polyline_to_path(&line);
        assert!(matches!(p.segments[0], PathSegment::MoveTo { .. }));
        assert_eq!(p.segments.len(), 3);
        assert!(!p.segments.iter().any(|s| matches!(s, PathSegment::Close)));
    }

    #[test]
    fn polyline_to_path_closed_appends_close() {
        let line = poly(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], true);
        let p = polyline_to_path(&line);
        assert!(matches!(p.segments.last(), Some(PathSegment::Close)));
    }
}

#[cfg(test)]
mod w15_offset_tests {
    use super::*;

    fn bounds(pts: &[(f32, f32)]) -> (f32, f32, f32, f32) {
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in pts {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        (minx, miny, maxx, maxy)
    }

    /// A sharp-cornered rect insets by EXACTLY `distance` on every edge
    /// (miter-correct: the 90° corners land at (10,10)/(190,90), not the
    /// `10/√2` an averaged-unit-normal offset would give).
    #[test]
    fn offset_closed_rect_inward_is_exact_at_corners() {
        let line = Polyline {
            // Closed rect, with the flatten-style trailing snap-back vertex.
            points: vec![
                (0.0, 0.0),
                (200.0, 0.0),
                (200.0, 100.0),
                (0.0, 100.0),
                (0.0, 0.0),
            ],
            closed: true,
        };
        let inw = offset_closed_outline(&line, 10.0, true);
        let (minx, miny, maxx, maxy) = bounds(&inw.points);
        assert!((minx - 10.0).abs() < 1e-3, "minx={minx}");
        assert!((miny - 10.0).abs() < 1e-3, "miny={miny}");
        assert!((maxx - 190.0).abs() < 1e-3, "maxx={maxx}");
        assert!((maxy - 90.0).abs() < 1e-3, "maxy={maxy}");
        // The trailing duplicate vertex is dropped → 4 distinct corners.
        assert_eq!(inw.points.len(), 4);
    }

    /// Outward offset grows the rect by exactly `distance` on every edge.
    #[test]
    fn offset_closed_rect_outward_is_exact_at_corners() {
        let line = Polyline {
            points: vec![(0.0, 0.0), (200.0, 0.0), (200.0, 100.0), (0.0, 100.0)],
            closed: true,
        };
        let outw = offset_closed_outline(&line, 10.0, false);
        let (minx, miny, maxx, maxy) = bounds(&outw.points);
        assert!((minx + 10.0).abs() < 1e-3, "minx={minx}");
        assert!((miny + 10.0).abs() < 1e-3, "miny={miny}");
        assert!((maxx - 210.0).abs() < 1e-3, "maxx={maxx}");
        assert!((maxy - 110.0).abs() < 1e-3, "maxy={maxy}");
    }

    /// Winding-agnostic: a counter-clockwise rect still insets (smaller
    /// area) for `inward = true`.
    #[test]
    fn offset_closed_inward_is_winding_agnostic() {
        // Counter-clockwise winding (reverse of the above).
        let ccw = Polyline {
            points: vec![(0.0, 0.0), (0.0, 100.0), (200.0, 100.0), (200.0, 0.0)],
            closed: true,
        };
        let inw = offset_closed_outline(&ccw, 10.0, true);
        let (minx, miny, maxx, maxy) = bounds(&inw.points);
        assert!(minx > 9.0 && maxx < 191.0, "inset x=({minx},{maxx})");
        assert!(miny > 9.0 && maxy < 91.0, "inset y=({miny},{maxy})");
    }

    /// A near-zero distance is a no-op (returns the input outline).
    #[test]
    fn offset_closed_zero_distance_is_noop() {
        let line = Polyline {
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            closed: true,
        };
        let off = offset_closed_outline(&line, 0.0, true);
        assert_eq!(off.points, line.points);
    }

    /// `polylines_to_path` emits one `MoveTo … Close` run per closed
    /// contour — a compound (donut) outline round-trips as one path.
    #[test]
    fn polylines_to_path_emits_per_contour_runs() {
        let outer = Polyline {
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            closed: true,
        };
        let inner = Polyline {
            points: vec![(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0)],
            closed: true,
        };
        let path = polylines_to_path(&[outer, inner]);
        let moves = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
            .count();
        let closes = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::Close))
            .count();
        assert_eq!(moves, 2, "two contours → two MoveTo");
        assert_eq!(closes, 2, "two closed contours → two Close");
    }
}

#[cfg(test)]
mod w15_polygon_path_offset_tests {
    use super::*;
    use paged_compose::{PathData, PathSegment};

    /// End-to-end on a polygon-style path: a rectangle encoded as cubic
    /// segments whose control points sit on the anchors (exactly how
    /// `polygon_path_from_anchors_with_open` serialises a straight-edged
    /// polygon). Flattening clusters samples near the corners; the
    /// collinear-simplify in `miter_offset_closed` must collapse those so
    /// the inward offset insets every edge by exactly `distance`.
    #[test]
    fn cubic_encoded_rect_insets_exactly() {
        let segs = vec![
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::CubicTo {
                cx1: 0.0,
                cy1: 0.0,
                cx2: 200.0,
                cy2: 0.0,
                x: 200.0,
                y: 0.0,
            },
            PathSegment::CubicTo {
                cx1: 200.0,
                cy1: 0.0,
                cx2: 200.0,
                cy2: 100.0,
                x: 200.0,
                y: 100.0,
            },
            PathSegment::CubicTo {
                cx1: 200.0,
                cy1: 100.0,
                cx2: 0.0,
                cy2: 100.0,
                x: 0.0,
                y: 100.0,
            },
            PathSegment::CubicTo {
                cx1: 0.0,
                cy1: 100.0,
                cx2: 0.0,
                cy2: 0.0,
                x: 0.0,
                y: 0.0,
            },
            PathSegment::Close,
        ];
        let lines = flatten_path(&PathData { segments: segs }, FLATTEN_STEPS);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        let inw = offset_closed_outline(&lines[0], 10.0, true);
        let (mut a, mut b, mut c, mut d) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in &inw.points {
            a = a.min(x);
            b = b.min(y);
            c = c.max(x);
            d = d.max(y);
        }
        assert!((a - 10.0).abs() < 1e-2, "min_x={a}");
        assert!((b - 10.0).abs() < 1e-2, "min_y={b}");
        assert!((c - 190.0).abs() < 1e-2, "max_x={c}");
        assert!((d - 90.0).abs() < 1e-2, "max_y={d}");
        // Collinear-collapsed back to four corners.
        assert_eq!(inw.points.len(), 4, "rect collapses to 4 corners");
    }
}
