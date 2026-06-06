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
//!     local outward normal by a signed distance (perpendicular
//!     offset). Used both for striped sub-rules and for stroke
//!     alignment (inside/outside) on open paths.
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
