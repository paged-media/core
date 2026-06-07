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

//! Frame-shape band intersection — wrap-INSIDE for non-rectangular
//! text frames (W1.10).
//!
//! A text frame whose `<PathGeometry>` is a polygon, oval, or compound
//! path (a contour with a hole) must lay its lines out so they conform
//! to the actual outline: short lines at the top/bottom of a circle,
//! wide in the middle; a donut's hole splits a middle line in two.
//!
//! This module is the *pure geometry* layer: given a [`FrameShape`]
//! (one or more closed contours in the frame's spread coordinates) and
//! a line's vertical band `[top, bottom]`, [`FrameShape::segments_in_band`]
//! returns the horizontal x-segments that lie *inside* the shape across
//! the whole band. The renderer feeds those segments — widest first —
//! into the same per-line `column_widths` + x-shift machinery the
//! wrap-AROUND-objects pass already drives (`build_perline_wrap_widths`
//! in `paged-renderer`), so wrap-inside reuses wrap-outside's line
//! plumbing rather than introducing a parallel path.
//!
//! Even-odd fill rule: contours are scanned independently and their
//! crossings merged, so an inner hole contour (wound either way)
//! carves the segment it overlaps — no special "this contour is a
//! hole" flag is needed. Compound paths fall out of the same code.

/// One closed contour as a flattened polyline. Curves (ovals, rounded
/// corners) are flattened to line segments before construction so the
/// scanline test is a pure polygon intersection.
pub type Contour = Vec<(f32, f32)>;

/// A frame outline as a set of closed contours, in the frame's spread
/// coordinates (already through the frame's `ItemTransform`). A single
/// contour is the common case (triangle, pentagon, oval); two or more
/// contours model a compound path — e.g. a donut (outer ring + inner
/// hole). Holes need no marker: the even-odd rule in
/// [`segments_in_band`](FrameShape::segments_in_band) carves them.
#[derive(Debug, Clone, Default)]
pub struct FrameShape {
    contours: Vec<Contour>,
}

impl FrameShape {
    /// Build from already-flattened contours. Contours with fewer than
    /// three points contribute no interior and are dropped.
    pub fn from_contours(contours: Vec<Contour>) -> Self {
        let contours = contours.into_iter().filter(|c| c.len() >= 3).collect();
        Self { contours }
    }

    /// `true` when the shape has no usable contour (caller should fall
    /// back to the AABB layout path).
    pub fn is_empty(&self) -> bool {
        self.contours.is_empty()
    }

    /// Borrow the contours (for the renderer's clip-path construction).
    pub fn contours(&self) -> &[Contour] {
        &self.contours
    }

    /// Inside x-intervals of the union of all contours at horizontal
    /// line `y`, paired by the even-odd rule and sorted left-to-right.
    ///
    /// Scanning every contour's edges into one crossing list and
    /// pairing them globally is exactly the even-odd fill rule, so a
    /// hole contour overlapping the outer one removes its span
    /// automatically (its two crossings split the outer interval).
    pub fn intervals_at_y(&self, y: f32) -> Vec<(f32, f32)> {
        let mut xs: Vec<f32> = Vec::new();
        for contour in &self.contours {
            scan_contour(contour, y, &mut xs);
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        pair_xs(&xs)
    }

    /// Available x-segments that lie inside the shape across the entire
    /// vertical band `[band_top, band_bottom]`.
    ///
    /// A glyph painted on this line occupies the full band (ascent
    /// above the baseline, descent below); for it to stay inside the
    /// outline the segment must be inside at *every* y in the band, not
    /// just at the baseline. We therefore intersect the interval sets
    /// at the band's top, middle, and bottom and keep only the overlap.
    /// Three samples are exact for convex contours (the extreme width
    /// is always at a band edge) and conservative for concave ones
    /// (a notch deeper than half the band could still poke a sliver in,
    /// but the polygon clip the renderer applies is the structural
    /// backstop — see `apply_polygon_clip`). Sampling the band rather
    /// than just the baseline is what makes top-of-circle lines come
    /// out shorter than middle lines.
    ///
    /// `band_top` is the smaller (upper) y; both are spread-coord pt.
    pub fn segments_in_band(&self, band_top: f32, band_bottom: f32) -> Vec<(f32, f32)> {
        if self.contours.is_empty() {
            return Vec::new();
        }
        let (lo, hi) = if band_top <= band_bottom {
            (band_top, band_bottom)
        } else {
            (band_bottom, band_top)
        };
        let mid = 0.5 * (lo + hi);
        // Nudge the edge samples inward by a hair so a band edge that
        // grazes a vertex doesn't drop the whole line on a rounding
        // tie.
        let eps = ((hi - lo) * 1e-3).max(1e-4);
        let top = self.intervals_at_y(lo + eps);
        let middle = self.intervals_at_y(mid);
        let bottom = self.intervals_at_y(hi - eps);
        let inter = intersect_intervals(&top, &middle);
        intersect_intervals(&inter, &bottom)
    }
}

/// Scanline crossings of one closed contour at horizontal line `y`,
/// appended to `xs`. The closing edge (last → first vertex) is
/// included. Edges parallel to `y` are skipped — their endpoints are
/// covered by the neighbouring edges. Half-open at the upper endpoint
/// (`y < hi_y`) so a shared vertex y is counted exactly once.
fn scan_contour(verts: &[(f32, f32)], y: f32, xs: &mut Vec<f32>) {
    let n = verts.len();
    if n < 2 {
        return;
    }
    for i in 0..n {
        let (x0, y0) = verts[i];
        let (x1, y1) = verts[(i + 1) % n];
        let (lo_y, hi_y, lo_x, hi_x) = if y0 <= y1 {
            (y0, y1, x0, x1)
        } else {
            (y1, y0, x1, x0)
        };
        if (lo_y - hi_y).abs() < 1e-9 {
            continue; // horizontal edge
        }
        if y < lo_y || y >= hi_y {
            continue;
        }
        let t = (y - lo_y) / (hi_y - lo_y);
        xs.push(lo_x + t * (hi_x - lo_x));
    }
}

/// Pair sorted crossings into inside intervals `[(x0,x1),(x2,x3),…]`.
/// An odd trailing crossing (numerical edge-grazing) is dropped.
/// Zero-width pairs are dropped — they carry no usable column.
fn pair_xs(xs: &[f32]) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(xs.len() / 2);
    let mut i = 0;
    while i + 1 < xs.len() {
        let (a, b) = (xs[i], xs[i + 1]);
        if b > a {
            out.push((a, b));
        }
        i += 2;
    }
    out
}

/// Intersect two sorted, disjoint interval lists. Returns the overlap
/// segments, sorted, disjoint. Both inputs must be left-to-right
/// sorted with no overlaps within a list (the output of `pair_xs`).
fn intersect_intervals(a: &[(f32, f32)], b: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if hi > lo {
            out.push((lo, hi));
        }
        // Advance whichever interval ends first.
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Flatten a cubic Bezier `p0 → (c1, c2) → p1` into a polyline,
/// appending the *intermediate* and end points to `out` (the caller
/// is responsible for having pushed `p0`). Uniform parameter sampling
/// at `steps` subdivisions; `steps` is chosen by the caller from a
/// flatness tolerance via [`cubic_steps_for_tolerance`].
pub fn flatten_cubic(
    p0: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    p1: (f32, f32),
    steps: u32,
    out: &mut Vec<(f32, f32)>,
) {
    let steps = steps.max(1);
    for k in 1..=steps {
        let t = k as f32 / steps as f32;
        let mt = 1.0 - t;
        let w0 = mt * mt * mt;
        let w1 = 3.0 * mt * mt * t;
        let w2 = 3.0 * mt * t * t;
        let w3 = t * t * t;
        out.push((
            w0 * p0.0 + w1 * c1.0 + w2 * c2.0 + w3 * p1.0,
            w0 * p0.1 + w1 * c1.1 + w2 * c2.1 + w3 * p1.1,
        ));
    }
}

/// Pick a subdivision count for a cubic Bezier so the flattened
/// polyline stays within `tol_pt` of the true curve. Uses the standard
/// control-polygon deviation bound: the maximum flattening error of an
/// `n`-segment uniform subdivision is ≤ `D / (8 n²)` where `D` is the
/// largest second-difference of the control points. Solving for `n`
/// and clamping keeps oval/rounded-corner outlines smooth without
/// over-tessellating nearly-straight segments (which collapse to 1).
pub fn cubic_steps_for_tolerance(
    p0: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    p1: (f32, f32),
    tol_pt: f32,
) -> u32 {
    // Second differences of the control points.
    let d1x = p0.0 - 2.0 * c1.0 + c2.0;
    let d1y = p0.1 - 2.0 * c1.1 + c2.1;
    let d2x = c1.0 - 2.0 * c2.0 + p1.0;
    let d2y = c1.1 - 2.0 * c2.1 + p1.1;
    let dmax = (d1x * d1x + d1y * d1y).max(d2x * d2x + d2y * d2y).sqrt();
    let tol = tol_pt.max(1e-3);
    if dmax <= 1e-6 {
        return 1; // straight (corner): no curvature to flatten.
    }
    // error ≤ 3 * dmax / (8 n²)  ⇒  n ≥ sqrt(3 dmax / (8 tol)).
    let n = (3.0 * dmax / (8.0 * tol)).sqrt().ceil() as u32;
    n.clamp(1, 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Approximate a unit-radius circle (centre `c`, radius `r`) as a
    /// 4-cubic-Bezier path flattened with our own helper — mirrors how
    /// IDML stores ovals (four cardinal anchors + 0.5523 handles).
    fn circle_contour(cx: f32, cy: f32, r: f32) -> Contour {
        // Kappa for a quarter-circle cubic approximation.
        const K: f32 = 0.552_284_8;
        let kr = K * r;
        // Cardinal anchors: top, right, bottom, left (clockwise).
        let top = (cx, cy - r);
        let right = (cx + r, cy);
        let bottom = (cx, cy + r);
        let left = (cx - r, cy);
        // Handles (right = outgoing, left = incoming) per anchor.
        let mut pts = vec![top];
        flatten_cubic(
            top,
            (cx + kr, cy - r),
            (cx + r, cy - kr),
            right,
            16,
            &mut pts,
        );
        flatten_cubic(
            right,
            (cx + r, cy + kr),
            (cx + kr, cy + r),
            bottom,
            16,
            &mut pts,
        );
        flatten_cubic(
            bottom,
            (cx - kr, cy + r),
            (cx - r, cy + kr),
            left,
            16,
            &mut pts,
        );
        flatten_cubic(
            left,
            (cx - r, cy - kr),
            (cx - kr, cy - r),
            top,
            16,
            &mut pts,
        );
        pts.pop(); // last point == first anchor; drop the duplicate.
        pts
    }

    fn width_of_widest(segs: &[(f32, f32)]) -> f32 {
        segs.iter()
            .map(|(a, b)| b - a)
            .fold(0.0_f32, |m, w| m.max(w))
    }

    #[test]
    fn triangle_band_widths_grow_downward() {
        // Apex-up triangle: narrow at top, wide at the base.
        // Vertices: apex (50, 0), base-left (0, 100), base-right (100, 100).
        let tri: Contour = vec![(50.0, 0.0), (0.0, 100.0), (100.0, 100.0)];
        let shape = FrameShape::from_contours(vec![tri]);

        // A thin band near the apex is much narrower than one near
        // the base.
        let top = shape.segments_in_band(8.0, 12.0);
        let bottom = shape.segments_in_band(88.0, 92.0);
        assert_eq!(top.len(), 1, "single segment near apex");
        assert_eq!(bottom.len(), 1, "single segment near base");
        let wt = width_of_widest(&top);
        let wb = width_of_widest(&bottom);
        assert!(
            wb > wt * 5.0,
            "base band ({wb}) should be far wider than apex band ({wt})"
        );
        // The base band should approach the full 100pt width.
        assert!(wb > 80.0, "base width {wb} near full triangle base");
    }

    #[test]
    fn circle_top_line_shorter_than_middle() {
        // r=100 circle centred at (100,100): spans y 0..200, x 0..200.
        let shape = FrameShape::from_contours(vec![circle_contour(100.0, 100.0, 100.0)]);
        // Near the top (small y) the chord is short; at the equator
        // it is the full diameter.
        let top = shape.segments_in_band(12.0, 16.0);
        let middle = shape.segments_in_band(98.0, 102.0);
        assert_eq!(top.len(), 1);
        assert_eq!(middle.len(), 1);
        let wt = width_of_widest(&top);
        let wm = width_of_widest(&middle);
        assert!(
            wm > wt + 40.0,
            "equator chord ({wm}) should be much wider than near-top chord ({wt})"
        );
        // Equator chord ≈ diameter (200pt), within flattening slop.
        assert!(wm > 190.0, "equator chord {wm} ≈ diameter");
        // The middle segment is centred on x=100.
        let (a, b) = middle[0];
        let centre = 0.5 * (a + b);
        assert!(
            (centre - 100.0).abs() < 1.0,
            "equator centred at x≈100, got {centre}"
        );
    }

    #[test]
    fn circle_band_stays_inside_outline() {
        // No part of any band segment may extend past the circle's
        // chord at the *narrower* band edge (the inside guarantee).
        let r = 100.0_f32;
        let shape = FrameShape::from_contours(vec![circle_contour(100.0, 100.0, r)]);
        for baseline in [30.0_f32, 60.0, 100.0, 140.0, 170.0] {
            let band_top = baseline - 12.0;
            let band_bottom = baseline + 4.0;
            let segs = shape.segments_in_band(band_top, band_bottom);
            // The narrowest chord over the band is at whichever edge is
            // farther from the equator.
            let far_y = if (band_top - 100.0).abs() > (band_bottom - 100.0).abs() {
                band_top
            } else {
                band_bottom
            };
            let dy = (far_y - 100.0).abs().min(r);
            let half_chord = (r * r - dy * dy).sqrt();
            for (a, b) in segs {
                assert!(
                    a >= 100.0 - half_chord - 1.0 && b <= 100.0 + half_chord + 1.0,
                    "segment ({a},{b}) at baseline {baseline} escaped chord ±{half_chord}"
                );
            }
        }
    }

    #[test]
    fn donut_hole_splits_middle_line() {
        // Outer square 0..200, inner hole square 60..140. A band
        // through the hole's y-range yields TWO segments (left of
        // hole, right of hole); a band above the hole yields one.
        let outer: Contour = vec![(0.0, 0.0), (200.0, 0.0), (200.0, 200.0), (0.0, 200.0)];
        let hole: Contour = vec![(60.0, 60.0), (140.0, 60.0), (140.0, 140.0), (60.0, 140.0)];
        let shape = FrameShape::from_contours(vec![outer, hole]);

        // Above the hole: one full-width segment.
        let above = shape.segments_in_band(20.0, 40.0);
        assert_eq!(above.len(), 1, "no hole above y=60");
        assert!((width_of_widest(&above) - 200.0).abs() < 1.0);

        // Through the hole's interior: two segments split by the hole.
        let through = shape.segments_in_band(95.0, 105.0);
        assert_eq!(through.len(), 2, "hole splits the line in two");
        let (l0, l1) = through[0];
        let (r0, r1) = through[1];
        assert!(
            (l0 - 0.0).abs() < 1.0 && (l1 - 60.0).abs() < 1.0,
            "left seg 0..60"
        );
        assert!(
            (r0 - 140.0).abs() < 1.0 && (r1 - 200.0).abs() < 1.0,
            "right seg 140..200"
        );
    }

    #[test]
    fn band_outside_shape_is_empty() {
        let tri: Contour = vec![(50.0, 0.0), (0.0, 100.0), (100.0, 100.0)];
        let shape = FrameShape::from_contours(vec![tri]);
        // A band entirely below the triangle's base.
        assert!(shape.segments_in_band(120.0, 130.0).is_empty());
        // A band entirely above the apex.
        assert!(shape.segments_in_band(-20.0, -10.0).is_empty());
    }

    #[test]
    fn flatten_steps_zero_for_straight_segment() {
        // Collinear control points ⇒ no curvature ⇒ a single step.
        let n = cubic_steps_for_tolerance((0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0), 0.1);
        assert_eq!(n, 1);
    }

    #[test]
    fn flatten_steps_grow_with_curvature() {
        // A sharply-bowed cubic needs several steps at a tight tol.
        let n =
            cubic_steps_for_tolerance((0.0, 0.0), (0.0, 100.0), (100.0, 100.0), (100.0, 0.0), 0.25);
        assert!(n > 4, "bowed cubic should subdivide, got {n}");
    }

    #[test]
    fn empty_shape_falls_back() {
        let shape = FrameShape::from_contours(vec![vec![(0.0, 0.0), (1.0, 1.0)]]);
        assert!(shape.is_empty(), "degenerate <3-point contour dropped");
        assert!(shape.segments_in_band(0.0, 1.0).is_empty());
    }
}
