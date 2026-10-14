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

//! A dashed or dotted stroke laid out the way InDesign lays it out.
//!
//! InDesign does not run one dash array around a path. Asked of InDesign
//! 20.0.1 (`stroke-styles`, read from its PDF by
//! `tools/indesign-export/pdf-strokes.py`; ADR 148), it treats every
//! anchor as a corner and every segment between two anchors as a side:
//!
//! * each side holds a whole number of periods, `n` = the side's length
//!   over the nominal period, a half rounding DOWN (2.5 → 2, 7.5 → 7);
//! * a dashed side shares what the rounding leaves between dash and gap
//!   equally (`Canned Dashed 3x2` at 5 pt, nominal 15/10, on a 61 pt
//!   side: 17.75/12.75); a dotted side gives it all to the gap;
//! * at a corner the two sides' dashes meet in one piece, half a dash
//!   along each (an L at a rectangle's corner); a dotted corner is a dot;
//! * an open path starts and ends on a whole dash, or on a dot.
//!
//! A curve is a side too: each quarter of a dashed ellipse gets its own
//! fitted array. The result is geometry in page space, so both
//! rasterizers and the PDF writer draw the same dashes.

use crate::display_list::{
    DisplayCommand, DisplayList, LineCap, Paint, PathData, PathId, PathSegment, Stroke,
    StrokeMarks, Transform,
};

/// A fitted stroke's pieces, in page space: the dashes inside the sides,
/// the pieces through the corners, and the dots. Any of them may be empty.
#[derive(Debug, Clone, Default)]
pub struct FittedDashes {
    pub dashes: PathData,
    pub corners: PathData,
    pub dots: PathData,
}

/// One side: the flattened points from one anchor to the next, with the
/// cumulative length at each.
pub(crate) struct Side {
    pub(crate) points: Vec<(f32, f32)>,
    cum: Vec<f32>,
}

impl Side {
    pub(crate) fn new(points: Vec<(f32, f32)>) -> Self {
        let mut cum = Vec::with_capacity(points.len());
        let mut total = 0.0f32;
        cum.push(0.0);
        for w in points.windows(2) {
            total += (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
            cum.push(total);
        }
        Side { points, cum }
    }

    pub(crate) fn len(&self) -> f32 {
        *self.cum.last().unwrap_or(&0.0)
    }

    fn locate(&self, s: f32) -> (usize, f32) {
        let s = s.clamp(0.0, self.len());
        let i = match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
            Ok(i) => i.min(self.points.len().saturating_sub(2)),
            Err(i) => i.saturating_sub(1).min(self.points.len().saturating_sub(2)),
        };
        let seg = self.cum[i + 1] - self.cum[i];
        (
            i,
            if seg > 1e-9 {
                (s - self.cum[i]) / seg
            } else {
                0.0
            },
        )
    }

    pub(crate) fn at(&self, s: f32) -> (f32, f32) {
        let (i, t) = self.locate(s);
        let (a, b) = (self.points[i], self.points[i + 1]);
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    }

    pub(crate) fn tangent(&self, s: f32) -> (f32, f32) {
        let (i, _) = self.locate(s);
        let (a, b) = (self.points[i], self.points[i + 1]);
        let l = (b.0 - a.0).hypot(b.1 - a.1);
        if l > 1e-9 {
            ((b.0 - a.0) / l, (b.1 - a.1) / l)
        } else {
            (1.0, 0.0)
        }
    }

    /// The points of the stretch `[s0, s1]`, ends interpolated.
    fn stretch(&self, s0: f32, s1: f32) -> Vec<(f32, f32)> {
        let mut out = vec![self.at(s0)];
        for (p, c) in self.points.iter().zip(&self.cum) {
            if *c > s0 && *c < s1 {
                out.push(*p);
            }
        }
        out.push(self.at(s1));
        out
    }
}

/// `x` rounded to the nearest whole number, a half going DOWN — what
/// InDesign does with the count of periods on a side.
fn round_half_down(x: f32) -> f32 {
    (x - 0.5).ceil()
}

/// Flatten one cubic, in page space, into `out` (start point excluded).
fn flatten_cubic(
    p0: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    p1: (f32, f32),
    out: &mut Vec<(f32, f32)>,
) {
    let rough = (c1.0 - p0.0).hypot(c1.1 - p0.1)
        + (c2.0 - c1.0).hypot(c2.1 - c1.1)
        + (p1.0 - c2.0).hypot(p1.1 - c2.1);
    let steps = ((rough / 1.5).ceil() as usize).clamp(8, 256);
    for k in 1..=steps {
        let t = k as f32 / steps as f32;
        let mt = 1.0 - t;
        let (a, b, c, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        out.push((
            a * p0.0 + b * c1.0 + c * c2.0 + d * p1.0,
            a * p0.1 + b * c1.1 + c * c2.1 + d * p1.1,
        ));
    }
}

/// The path's contours as sides, in page space, each with its closed flag.
pub(crate) fn contours(path: &PathData, transform: &Transform) -> Vec<(Vec<Side>, bool)> {
    let mut out = Vec::new();
    let mut sides: Vec<Side> = Vec::new();
    let mut start = (0.0f32, 0.0f32);
    let mut pen = start;
    let push_side = |sides: &mut Vec<Side>, pts: Vec<(f32, f32)>| {
        let side = Side::new(pts);
        if side.len() > 1e-4 {
            sides.push(side);
        }
    };
    for seg in &path.segments {
        match *seg {
            PathSegment::MoveTo { x, y } => {
                if !sides.is_empty() {
                    out.push((std::mem::take(&mut sides), false));
                }
                start = transform.apply(x, y);
                pen = start;
            }
            PathSegment::LineTo { x, y } => {
                let p = transform.apply(x, y);
                push_side(&mut sides, vec![pen, p]);
                pen = p;
            }
            PathSegment::QuadTo { cx, cy, x, y } => {
                let c = transform.apply(cx, cy);
                let p = transform.apply(x, y);
                let c1 = (
                    pen.0 + 2.0 / 3.0 * (c.0 - pen.0),
                    pen.1 + 2.0 / 3.0 * (c.1 - pen.1),
                );
                let c2 = (p.0 + 2.0 / 3.0 * (c.0 - p.0), p.1 + 2.0 / 3.0 * (c.1 - p.1));
                let mut pts = vec![pen];
                flatten_cubic(pen, c1, c2, p, &mut pts);
                push_side(&mut sides, pts);
                pen = p;
            }
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                let c1 = transform.apply(cx1, cy1);
                let c2 = transform.apply(cx2, cy2);
                let p = transform.apply(x, y);
                let mut pts = vec![pen];
                flatten_cubic(pen, c1, c2, p, &mut pts);
                push_side(&mut sides, pts);
                pen = p;
            }
            PathSegment::Close => {
                if (pen.0 - start.0).hypot(pen.1 - start.1) > 1e-4 {
                    push_side(&mut sides, vec![pen, start]);
                }
                pen = start;
                if !sides.is_empty() {
                    out.push((std::mem::take(&mut sides), true));
                }
            }
        }
    }
    if !sides.is_empty() {
        out.push((sides, false));
    }
    out
}

fn push_polyline(path: &mut PathData, pts: &[(f32, f32)]) {
    let mut it = pts.iter();
    if let Some(&(x, y)) = it.next() {
        path.segments.push(PathSegment::MoveTo { x, y });
        for &(x, y) in it {
            path.segments.push(PathSegment::LineTo { x, y });
        }
    }
}

/// A dot: a hair of a segment along the path, so a round cap draws it
/// on every rasterizer (a zero-length segment has no direction to cap).
fn push_dot(path: &mut PathData, p: (f32, f32), t: (f32, f32)) {
    const HAIR_PT: f32 = 1e-3;
    path.segments.push(PathSegment::MoveTo { x: p.0, y: p.1 });
    path.segments.push(PathSegment::LineTo {
        x: p.0 + t.0 * HAIR_PT,
        y: p.1 + t.1 * HAIR_PT,
    });
}

/// Lay `pattern` (a nominal `[dash, gap]` in pt; a zero dash is a dot)
/// out over `path` mapped by `transform`, the way InDesign does. `None`
/// for a pattern this does not model (not two entries, or no period),
/// which then strokes with the rasterizer's own dashing.
pub fn fit_dashes(path: &PathData, transform: &Transform, pattern: &[f32]) -> Option<FittedDashes> {
    let [d0, g0] = *pattern else {
        return None;
    };
    if !(d0.is_finite() && g0.is_finite()) || d0 < 0.0 || g0 < 0.0 || d0 + g0 <= 1e-3 {
        return None;
    }
    let dotted = d0 == 0.0;
    let mut out = FittedDashes::default();
    for (sides, closed) in contours(path, transform) {
        let count = sides.len();
        // The fitted dash on each side, for the corner pieces.
        let mut fitted_dash = vec![0.0f32; count];
        for (i, side) in sides.iter().enumerate() {
            let l = side.len();
            let open_start = !closed && i == 0;
            let open_end = !closed && i + 1 == count;
            if dotted {
                let n = round_half_down(l / g0).max(1.0);
                let g = l / n;
                for k in 1..n as usize {
                    let s = k as f32 * g;
                    push_dot(&mut out.dots, side.at(s), side.tangent(s));
                }
                continue;
            }
            // Ends: half a dash at a corner, a whole one at an open end.
            let a = if open_start { 1.0 } else { 0.5 };
            let b = if open_end { 1.0 } else { 0.5 };
            let ends = a + b - 1.0;
            let m = round_half_down((l - ends * d0) / (d0 + g0)).max(1.0);
            let delta = (l - (m + ends) * d0 - m * g0) / (2.0 * m + ends);
            let d = (d0 + delta).max(0.0);
            let g = (g0 + delta).max(0.0);
            fitted_dash[i] = d;
            let mut s = if open_start { 0.0 } else { d * 0.5 + g };
            let stop = if open_end { l } else { l - d * 0.5 };
            while s + d <= stop + 1e-3 && d > 0.0 {
                push_polyline(&mut out.dashes, &side.stretch(s, (s + d).min(l)));
                s += d + g;
            }
        }
        // Anchors: corners (and, for dots, the open ends too).
        for i in 0..count {
            let first = i == 0;
            if dotted {
                let side = &sides[i];
                push_dot(&mut out.dots, side.at(0.0), side.tangent(0.0));
                if !closed && i + 1 == count {
                    let l = side.len();
                    push_dot(&mut out.dots, side.at(l), side.tangent(l));
                }
                continue;
            }
            if !closed && first {
                continue;
            }
            let prev = if first { count - 1 } else { i - 1 };
            let (inc, out_side) = (&sides[prev], &sides[i]);
            let li = inc.len();
            let mut pts = inc.stretch((li - fitted_dash[prev] * 0.5).max(0.0), li);
            let tail = out_side.stretch(0.0, (fitted_dash[i] * 0.5).min(out_side.len()));
            pts.extend(tail.into_iter().skip(1));
            push_polyline(&mut out.corners, &pts);
        }
    }
    Some(out)
}

/// Push a stroke of `path_id` under `transform`. A dashed stroke is laid
/// out the way InDesign lays it out ([`fit_dashes`]) and pushed as page-
/// space geometry with no dash; anything else is pushed as it is.
pub fn push_stroke(
    list: &mut DisplayList,
    path_id: PathId,
    stroke: Stroke,
    paint: Paint,
    transform: Transform,
) {
    if stroke.dash.marks != StrokeMarks::None {
        if let Some(path) = list.paths.get(path_id) {
            let marks =
                crate::stroke_marks::fit_marks(path, &transform, stroke.dash.marks, stroke.width);
            crate::stroke_marks::push_marks(list, marks, paint);
            return;
        }
    }
    let fitted = if stroke.dash.is_solid() {
        None
    } else {
        list.paths
            .get(path_id)
            .and_then(|p| fit_dashes(p, &transform, stroke.dash.as_slice()))
    };
    let Some(fitted) = fitted else {
        list.push(DisplayCommand::StrokePath {
            path_id,
            paint,
            stroke,
            transform,
        });
        return;
    };
    let mut solid = stroke;
    solid.dash = Default::default();
    for (piece, cap) in [
        (fitted.corners, solid.cap),
        (fitted.dashes, solid.cap),
        (fitted.dots, LineCap::Round),
    ] {
        if piece.segments.is_empty() {
            continue;
        }
        let id = list.paths.push_anon(piece);
        let mut s = solid;
        s.cap = cap;
        list.push(DisplayCommand::StrokePath {
            path_id: id,
            paint,
            stroke: s,
            transform: Transform::IDENTITY,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: f32, h: f32) -> PathData {
        // InDesign's order: down the left edge first.
        PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: 0.0, y: h },
                PathSegment::LineTo { x: w, y: h },
                PathSegment::LineTo { x: w, y: 0.0 },
                PathSegment::Close,
            ],
        }
    }

    fn line(l: f32) -> PathData {
        PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: l, y: 0.0 },
            ],
        }
    }

    /// Dash starts along the first contour of each piece.
    fn starts(p: &PathData) -> Vec<(f32, f32)> {
        p.segments
            .iter()
            .filter_map(|s| match s {
                PathSegment::MoveTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect()
    }

    /// `stroke-styles` row 1, column b: `Dashed` (12/12) on a 137 × 61
    /// rectangle. InDesign: the 61 pt sides fit 10.167/10.167 with one
    /// dash from 15.25 to 25.417, the 137 pt sides 11.417/11.417 from
    /// 17.125; L-shaped corners half a dash along each side.
    #[test]
    fn dashed_sides_fit_whole_periods() {
        let f = fit_dashes(&rect(137.0, 61.0), &Transform::IDENTITY, &[12.0, 12.0]).unwrap();
        let s = starts(&f.dashes);
        // Left side: dashes at y 15.25 and 35.583.
        assert!((s[0].1 - 15.25).abs() < 1e-2, "{s:?}");
        assert!((s[1].1 - 35.583).abs() < 1e-2, "{s:?}");
        // Bottom side's first dash at x 17.125.
        let bottom = s.iter().find(|p| (p.1 - 61.0).abs() < 1e-3).unwrap();
        assert!((bottom.0 - 17.125).abs() < 1e-2, "{s:?}");
        // Four corners, the first ending 5.708 along the bottom.
        assert_eq!(starts(&f.corners).len(), 4);
        assert!(f.dots.segments.is_empty());
    }

    /// Row 4, column a: `Canned Dotted` at 2 pt (nominal 0/6) on 120 × 60:
    /// 10 and 20 periods; a dot at every corner.
    #[test]
    fn dots_fit_with_a_dot_at_each_corner() {
        let f = fit_dashes(&rect(120.0, 60.0), &Transform::IDENTITY, &[0.0, 6.0]).unwrap();
        // 9 + 19 + 9 + 19 interior dots + 4 corners.
        assert_eq!(starts(&f.dots).len(), 9 + 19 + 9 + 19 + 4);
        assert!(f.dashes.segments.is_empty() && f.corners.segments.is_empty());
    }

    /// Row 1, column c: `Dashed` on an open 150 pt line: whole dashes at
    /// both ends, 7 dashes of 11.538.
    #[test]
    fn an_open_line_ends_on_whole_dashes() {
        let f = fit_dashes(&line(150.0), &Transform::IDENTITY, &[12.0, 12.0]).unwrap();
        let s = starts(&f.dashes);
        assert_eq!(s.len(), 7);
        assert!((s[1].0 - 23.077).abs() < 1e-2, "{s:?}");
        assert!(f.corners.segments.is_empty());
        // Japanese Dots (0/8) on the same line: 19 periods, 20 dots.
        let f = fit_dashes(&line(150.0), &Transform::IDENTITY, &[0.0, 8.0]).unwrap();
        assert_eq!(starts(&f.dots).len(), 20);
    }

    /// A half rounds down: `Canned Dotted` at 4 pt on 150 pt is 12.5
    /// periods, and InDesign makes it 12.
    #[test]
    fn a_half_period_rounds_down() {
        let f = fit_dashes(&line(150.0), &Transform::IDENTITY, &[0.0, 12.0]).unwrap();
        assert_eq!(starts(&f.dots).len(), 13);
    }

    #[test]
    fn patterns_it_does_not_model_are_left_alone() {
        assert!(fit_dashes(&line(10.0), &Transform::IDENTITY, &[1.0, 2.0, 3.0, 4.0]).is_none());
        assert!(fit_dashes(&line(10.0), &Transform::IDENTITY, &[0.0, 0.0]).is_none());
    }
}
