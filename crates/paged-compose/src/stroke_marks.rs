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

//! InDesign's hash and diamond stroke styles, laid out the way InDesign
//! lays them out.
//!
//! Asked of InDesign 20.0.1 (`stroke-styles` rows 6–9, read from its PDF
//! by `tools/indesign-export/pdf-strokes.py`; ADR 150). Unlike dashes
//! (ADR 148), which fit each side on its own, marks run along the whole
//! contour, corners and all:
//!
//! * marks sit at a nominal pitch — half the weight for a hash, the
//!   weight for a diamond — fitted so that a whole number of pitches,
//!   rounding DOWN, lies between the first and the last mark; those two
//!   are inset from the contour's ends by half what they paint along the
//!   path, and a closed contour's ends are its first anchor;
//! * a hash is a hairline of 0.187 × the weight with butt caps. A
//!   straight hash crosses the path square, the full weight; a slant hash
//!   leans 45° so its painted ends stay inside the weight, and each end
//!   sits at its own place along the path, so a hash through a corner
//!   bends round it;
//! * a white diamond is an outlined rhombus, 0.09 × the weight wide,
//!   0.876 × the weight across the path and 0.92 × the fitted pitch along
//!   it; one through a corner lies along the chord between its tips.
//!
//! Measured with the default butt cap and miter join. The geometry is in
//! page space, so both rasterizers and the PDF writer draw the same marks.

use crate::dash_fit::{contours, Side};
use crate::display_list::{
    DisplayCommand, DisplayList, LineCap, LineJoin, Paint, PathData, PathSegment, Stroke,
    StrokeMarks, Transform,
};

/// A hash's pen, as a fraction of the stroke weight.
const HASH_PEN: f32 = 0.187;
/// A diamond's pen, as a fraction of the stroke weight.
const DIAMOND_PEN: f32 = 0.09;
/// A diamond's half-height across the path, as a fraction of the weight.
const DIAMOND_HALF_ACROSS: f32 = 0.438;
/// A diamond's half-length along the path, as a fraction of the pitch.
const DIAMOND_HALF_ALONG: f32 = 0.4598;
/// The first and last diamonds' centres from the ends, as a fraction of
/// the pitch: half of what a diamond paints along the path.
const DIAMOND_INSET: f32 = 0.5248;

/// The marks a stroke style lays along a path, in page space, with the
/// pen each is drawn with.
#[derive(Debug, Clone, Default)]
pub struct FittedMarks {
    /// Open two-point segments, drawn with butt caps.
    pub hashes: PathData,
    /// Closed quadrilaterals, drawn with miter joins.
    pub diamonds: PathData,
    pub hash_pen: f32,
    pub diamond_pen: f32,
}

/// One contour as a single arclength track: its sides end to end.
fn track(sides: &[Side]) -> Side {
    let mut points: Vec<(f32, f32)> = Vec::new();
    for side in sides {
        let skip = usize::from(!points.is_empty());
        points.extend(side.points.iter().skip(skip).copied());
    }
    Side::new(points)
}

/// The left normal of travel in y-down page space, `(dy, −dx)`: the side
/// InDesign leans a slant hash's leading end towards (and the side a
/// Wavy stroke leaves its start on).
fn normal(t: (f32, f32)) -> (f32, f32) {
    (t.1, -t.0)
}

fn offset(p: (f32, f32), n: (f32, f32), d: f32) -> (f32, f32) {
    (p.0 + n.0 * d, p.1 + n.1 * d)
}

/// Centres of `count` marks spread evenly from `inset` to `len − inset`
/// (one mark at the middle when the track is too short for two).
fn centres(len: f32, inset: f32, pitch: f32) -> Vec<f32> {
    let room = len - 2.0 * inset;
    if room <= 0.0 || pitch <= 0.0 {
        return if len > 0.0 {
            vec![len * 0.5]
        } else {
            Vec::new()
        };
    }
    let intervals = (room / pitch + 1e-3).floor();
    if intervals < 1.0 {
        return vec![len * 0.5];
    }
    let step = room / intervals;
    (0..=intervals as usize)
        .map(|k| inset + k as f32 * step)
        .collect()
}

/// Lay `marks` of a stroke `weight` pt wide along `path` mapped by
/// `transform`, the way InDesign does.
pub fn fit_marks(
    path: &PathData,
    transform: &Transform,
    marks: StrokeMarks,
    weight: f32,
) -> FittedMarks {
    let mut out = FittedMarks {
        hash_pen: HASH_PEN * weight,
        diamond_pen: DIAMOND_PEN * weight,
        ..FittedMarks::default()
    };
    if weight <= 0.0 {
        return out;
    }
    let half = weight * 0.5;
    let pen = out.hash_pen;
    // A slant hash's ends sit this far along and across from its centre,
    // so its butt-capped pen stays within the weight.
    let lean = half - pen * 0.5 * std::f32::consts::FRAC_1_SQRT_2;
    for (sides, _closed) in contours(path, transform) {
        let t = track(&sides);
        let len = t.len();
        if len <= 1e-4 {
            continue;
        }
        let at = |s: f32| (t.at(s), normal(t.tangent(s)));
        let mut segment = |a: (f32, f32), b: (f32, f32)| {
            out.hashes
                .segments
                .push(PathSegment::MoveTo { x: a.0, y: a.1 });
            out.hashes
                .segments
                .push(PathSegment::LineTo { x: b.0, y: b.1 });
        };
        match marks {
            StrokeMarks::None => {}
            StrokeMarks::StraightHash => {
                for s in centres(len, pen * 0.5, half) {
                    let (p, n) = at(s);
                    segment(offset(p, n, half), offset(p, n, -half));
                }
            }
            StrokeMarks::LeftSlantHash | StrokeMarks::RightSlantHash => {
                let left = marks == StrokeMarks::LeftSlantHash;
                for s in centres(len, half, half) {
                    // Each end at its own place along the path.
                    let (lead, trail) = if left {
                        (s - lean, s + lean)
                    } else {
                        (s + lean, s - lean)
                    };
                    let (p0, n0) = at(lead);
                    let (p1, n1) = at(trail);
                    segment(offset(p0, n0, lean), offset(p1, n1, -lean));
                }
            }
            StrokeMarks::WhiteDiamond => {
                let room = len - 2.0 * DIAMOND_INSET * weight;
                let intervals = if room > 0.0 {
                    (room / weight + 1e-3).floor()
                } else {
                    0.0
                };
                let pitch = len / (intervals + 2.0 * DIAMOND_INSET);
                let along = DIAMOND_HALF_ALONG * pitch;
                let across = DIAMOND_HALF_ACROSS * weight;
                for k in 0..=intervals as usize {
                    let s = DIAMOND_INSET * pitch + k as f32 * pitch;
                    let (a, b) = (t.at(s - along), t.at(s + along));
                    let chord = (b.0 - a.0).hypot(b.1 - a.1);
                    if chord <= 1e-6 {
                        continue;
                    }
                    let u = ((b.0 - a.0) / chord, (b.1 - a.1) / chord);
                    let n = normal(u);
                    let c = ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
                    let h = chord * 0.5;
                    let corners = [
                        offset(c, u, -h),
                        offset(c, n, across),
                        offset(c, u, h),
                        offset(c, n, -across),
                    ];
                    let d = &mut out.diamonds.segments;
                    d.push(PathSegment::MoveTo {
                        x: corners[0].0,
                        y: corners[0].1,
                    });
                    for q in &corners[1..] {
                        d.push(PathSegment::LineTo { x: q.0, y: q.1 });
                    }
                    d.push(PathSegment::Close);
                }
            }
        }
    }
    out
}

/// Push `marks` as page-space strokes in `paint`.
pub fn push_marks(list: &mut DisplayList, marks: FittedMarks, paint: Paint) {
    for (piece, pen, cap) in [
        (marks.hashes, marks.hash_pen, LineCap::Butt),
        (marks.diamonds, marks.diamond_pen, LineCap::Butt),
    ] {
        if piece.segments.is_empty() || pen <= 0.0 {
            continue;
        }
        let id = list.paths.push_anon(piece);
        let mut stroke = Stroke::new(pen);
        stroke.cap = cap;
        stroke.join = LineJoin::Miter;
        stroke.miter_limit = 10.0;
        list.push(DisplayCommand::StrokePath {
            path_id: id,
            paint,
            stroke,
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

    /// Each mark's points (a hash's two ends, a diamond's four corners).
    fn marks(p: &PathData) -> Vec<Vec<(f32, f32)>> {
        let mut out: Vec<Vec<(f32, f32)>> = Vec::new();
        for s in &p.segments {
            match *s {
                PathSegment::MoveTo { x, y } => out.push(vec![(x, y)]),
                PathSegment::LineTo { x, y } => out.last_mut().unwrap().push((x, y)),
                _ => {}
            }
        }
        out
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 2e-3 && (a.1 - b.1).abs() < 2e-3
    }

    /// `stroke-styles` row 6, column a: `Left Slant Hash` at 2 pt on
    /// 120 × 60. InDesign: 359 hashes 1 pt apart from 1 pt in, pen 0.374;
    /// the first runs (0.868, −0.132) → (−0.868, 1.868) from the corner
    /// (page y down), and the one at the bottom-left corner bends round
    /// it: (0.868, 59.132) → (0.868, 60.868).
    #[test]
    fn left_slant_hashes_run_along_the_whole_contour() {
        let f = fit_marks(
            &rect(120.0, 60.0),
            &Transform::IDENTITY,
            StrokeMarks::LeftSlantHash,
            2.0,
        );
        assert!((f.hash_pen - 0.374).abs() < 1e-4);
        let m = marks(&f.hashes);
        assert_eq!(m.len(), 359);
        assert!(near(m[0][0], (0.868, 0.132)), "{:?}", m[0]);
        assert!(near(m[0][1], (-0.868, 1.868)), "{:?}", m[0]);
        assert!(near(m[59][0], (0.868, 59.132)), "{:?}", m[59]);
        assert!(near(m[59][1], (0.868, 60.868)), "{:?}", m[59]);
    }

    /// Row 6, column b: 137 × 61 at 5 pt — 157 hashes from 2.5 pt in, the
    /// pitch fitted to 391 / 156; the 24th crosses the bottom-left corner
    /// with its trailing end on the bottom side (InDesign: (2.169,
    /// 60.485) → (3.823, 63.17)).
    #[test]
    fn a_slant_hash_through_a_corner_keeps_each_end_on_its_side() {
        let f = fit_marks(
            &rect(137.0, 61.0),
            &Transform::IDENTITY,
            StrokeMarks::LeftSlantHash,
            5.0,
        );
        let m = marks(&f.hashes);
        assert_eq!(m.len(), 157);
        assert!(near(m[24][0], (2.169, 60.485)), "{:?}", m[24]);
        assert!(near(m[24][1], (3.823, 63.170)), "{:?}", m[24]);
    }

    /// Row 7 mirrors row 6; row 8, `Straight Hash`, crosses the path
    /// square from half its pen in: 360 marks on 120 × 60 at 2 pt, 159 on
    /// 137 × 61 at 5 pt, 75 on a 150 pt line at 4 pt.
    #[test]
    fn straight_and_right_slant_hashes() {
        let f = fit_marks(
            &rect(120.0, 60.0),
            &Transform::IDENTITY,
            StrokeMarks::RightSlantHash,
            2.0,
        );
        let m = marks(&f.hashes);
        assert!(near(m[0][0], (0.868, 1.868)), "{:?}", m[0]);
        assert!(near(m[0][1], (-0.868, 0.132)), "{:?}", m[0]);
        for (path, w, n) in [
            (rect(120.0, 60.0), 2.0, 360),
            (rect(137.0, 61.0), 5.0, 159),
            (line(150.0), 4.0, 75),
        ] {
            let f = fit_marks(&path, &Transform::IDENTITY, StrokeMarks::StraightHash, w);
            assert_eq!(marks(&f.hashes).len(), n);
        }
        let f = fit_marks(
            &line(150.0),
            &Transform::IDENTITY,
            StrokeMarks::StraightHash,
            4.0,
        );
        let m = marks(&f.hashes);
        assert!(near(m[0][0], (0.374, -2.0)), "{:?}", m[0]);
        assert!(near(m[74][0], (149.626, -2.0)), "{:?}", m[74]);
    }

    /// Row 9, `White Diamond`: 179 on 120 × 60 at 2 pt, 79 on 137 × 61 at
    /// 5 pt, 37 on the 150 pt line at 4 pt; on the line InDesign's first
    /// runs 0.263 → 3.986 along and ±1.753 across, pen 0.36.
    #[test]
    fn white_diamonds_fit_the_weight() {
        for (path, w, n) in [
            (rect(120.0, 60.0), 2.0, 179),
            (rect(137.0, 61.0), 5.0, 79),
            (line(150.0), 4.0, 37),
        ] {
            let f = fit_marks(&path, &Transform::IDENTITY, StrokeMarks::WhiteDiamond, w);
            assert_eq!(marks(&f.diamonds).len(), n);
        }
        let f = fit_marks(
            &line(150.0),
            &Transform::IDENTITY,
            StrokeMarks::WhiteDiamond,
            4.0,
        );
        assert!((f.diamond_pen - 0.36).abs() < 1e-4);
        let first = &marks(&f.diamonds)[0];
        assert!(near(first[0], (0.263, 0.0)), "{first:?}");
        assert!(near(first[2], (3.986, 0.0)), "{first:?}");
        assert!((first[1].1.abs() - 1.753).abs() < 2e-3, "{first:?}");
    }
}
