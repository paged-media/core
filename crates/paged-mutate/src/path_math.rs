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

//! Pure geometry helpers for path-topology ops (Track J) and
//! affine composition (Track L group rebase).
//!
//! Track J:
//!   - `split_segment_de_casteljau` — inserts a new anchor on a
//!     cubic Bezier segment at parameter `t` without altering the
//!     curve's visible shape.
//!   - `smooth_handles_from_neighbours` — derives a smooth (left,
//!     right) pair for an anchor from its neighbours, 1/3-distance
//!     heuristic.
//!
//! Track L:
//!   - `affine_multiply` / `affine_inverse` / `affine_identity` —
//!     2D affine matrix algebra on IDML's `[a, b, c, d, tx, ty]`
//!     packing. The group-rebase math needs `delta = G' * inv(G)`
//!     to lift each leaf's pre-baked transform into the new group
//!     coords without visually shifting the rendered output.
//!
//! All math runs in the path / shape's local coordinate system.
//! No clamping, no document state — callers compose with their own
//! bookkeeping.

use crate::operation::PathAnchorSpec;

type P = [f32; 2];

fn lerp(a: P, b: P, t: f32) -> P {
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: P, k: f32) -> P {
    [a[0] * k, a[1] * k]
}

fn length(a: P) -> f32 {
    (a[0] * a[0] + a[1] * a[1]).sqrt()
}

/// Result of splitting a cubic Bezier segment at parameter `t`. Each
/// field names which anchor / handle changes on the resulting two-
/// segment path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentSplit {
    /// New `right` handle for the segment's start anchor (was the
    /// outgoing tangent into `t = 0`'s segment, now ends at `t`'s
    /// midpoint).
    pub start_right: P,
    /// New `left` handle for the inserted anchor.
    pub mid_left: P,
    /// Position of the inserted anchor (the on-curve point at `t`).
    pub mid_anchor: P,
    /// New `right` handle for the inserted anchor.
    pub mid_right: P,
    /// New `left` handle for the segment's end anchor.
    pub end_left: P,
}

/// Split a cubic Bezier from `start` → `end` (with start's outgoing
/// handle `start_right` and end's incoming handle `end_left`) at
/// parameter `t ∈ [0, 1]`. Returns the new mid-anchor + the four
/// adjusted handles. The resulting two-segment path traces the
/// identical curve.
///
/// Standard de Casteljau construction:
///   Q0 = lerp(start, start_right, t)
///   Q1 = lerp(start_right, end_left, t)
///   Q2 = lerp(end_left, end, t)
///   R0 = lerp(Q0, Q1, t)
///   R1 = lerp(Q1, Q2, t)
///   M  = lerp(R0, R1, t)   ← inserted anchor
/// New handles map: start_right'=Q0, mid_left=R0, mid_right=R1,
/// end_left'=Q2.
pub fn split_segment_de_casteljau(
    start: P,
    start_right: P,
    end_left: P,
    end: P,
    t: f32,
) -> SegmentSplit {
    let q0 = lerp(start, start_right, t);
    let q1 = lerp(start_right, end_left, t);
    let q2 = lerp(end_left, end, t);
    let r0 = lerp(q0, q1, t);
    let r1 = lerp(q1, q2, t);
    let m = lerp(r0, r1, t);
    SegmentSplit {
        start_right: q0,
        mid_left: r0,
        mid_anchor: m,
        mid_right: r1,
        end_left: q2,
    }
}

/// Compute smooth (left, right) handles for an anchor at `curr`
/// based on the neighbouring on-curve points `prev` and `next`.
///
/// Standard heuristic: the tangent direction at `curr` is the unit
/// vector from `prev` toward `next`. Handle lengths are 1/3 of the
/// distance to the respective neighbour along that tangent.
///
/// Falls back to corner handles (left = right = curr) when `prev`
/// or `next` coincides with `curr` so the tangent isn't defined.
pub fn smooth_handles_from_neighbours(prev: P, curr: P, next: P) -> (P, P) {
    let tangent = sub(next, prev);
    let tan_len = length(tangent);
    if tan_len < 1e-6 {
        return (curr, curr);
    }
    let unit = scale(tangent, 1.0 / tan_len);
    let prev_dist = length(sub(curr, prev));
    let next_dist = length(sub(next, curr));
    let left = sub(curr, scale(unit, prev_dist / 3.0));
    let right = add(curr, scale(unit, next_dist / 3.0));
    (left, right)
}

/// Build a `PathAnchorSpec` for the new mid-anchor of a segment
/// split. The split's `mid_left` / `mid_right` become the anchor's
/// `left` / `right` handles; `mid_anchor` becomes the on-curve
/// point.
pub fn anchor_from_split(split: SegmentSplit) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: split.mid_anchor,
        left: split.mid_left,
        right: split.mid_right,
    }
}

/// Editor-ops (Pencil) — fit a sampled polyline to smooth cubic
/// Beziers (flo_curves' Schneider fitter) and convert the fitted
/// segments back to `PathAnchorSpec`s: anchor `i` carries segment
/// `i-1`'s incoming control as `left` and segment `i`'s outgoing
/// control as `right`. Falls back to the input (corner anchors) when
/// the fitter declines. The tolerance is in document pt — the editor
/// pre-simplifies the raw pointer samples (RDP, camera-scaled) before
/// sending, so this trades anchor count against shape error on an
/// already-clean polyline.
pub fn fit_polyline_to_anchors(points: &[PathAnchorSpec]) -> Vec<PathAnchorSpec> {
    use flo_curves::bezier::Curve;
    use flo_curves::Coord2;

    const FIT_TOLERANCE_PT: f64 = 1.0;

    if points.len() < 3 {
        return points.to_vec();
    }
    let coords: Vec<Coord2> = points
        .iter()
        .map(|a| Coord2(f64::from(a.anchor[0]), f64::from(a.anchor[1])))
        .collect();
    let curves = match flo_curves::bezier::fit_curve::<Curve<Coord2>>(&coords, FIT_TOLERANCE_PT) {
        Some(curves) if !curves.is_empty() => curves,
        _ => return points.to_vec(),
    };
    let p = |c: Coord2| [c.0 as f32, c.1 as f32];
    let mut anchors: Vec<PathAnchorSpec> = Vec::with_capacity(curves.len() + 1);
    let first = p(curves[0].start_point);
    anchors.push(PathAnchorSpec {
        anchor: first,
        left: first, // endpoints carry no incoming handle
        right: p(curves[0].control_points.0),
    });
    for (i, curve) in curves.iter().enumerate() {
        let end = p(curve.end_point);
        let right = curves
            .get(i + 1)
            .map(|next| p(next.control_points.0))
            .unwrap_or(end);
        anchors.push(PathAnchorSpec {
            anchor: end,
            left: p(curve.control_points.1),
            right,
        });
    }
    anchors
}

// ---------------------------------------------------------------------------
// Track L — affine matrix algebra
// ---------------------------------------------------------------------------

/// IDML `ItemTransform` packing: `[a, b, c, d, tx, ty]` representing
/// the 2×3 affine matrix `| a c tx |` over `| b d ty |`. A point
/// `(x, y)` maps to `(a*x + c*y + tx, b*x + d*y + ty)`.
pub type Affine = [f32; 6];

pub const AFFINE_IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Compose two 2D affines: `M = A * B`. Apply order is "B first,
/// then A": a point `p` maps to `A(B(p))`. Matches the IDML
/// item_transform composition convention.
pub fn affine_multiply(a: Affine, b: Affine) -> Affine {
    // | a0 a2 a4 |   | b0 b2 b4 |
    // | a1 a3 a5 | * | b1 b3 b5 |
    // | 0  0  1  |   | 0  0  1  |
    [
        a[0] * b[0] + a[2] * b[1],          // m0 = a*b0 + c*b1
        a[1] * b[0] + a[3] * b[1],          // m1
        a[0] * b[2] + a[2] * b[3],          // m2
        a[1] * b[2] + a[3] * b[3],          // m3
        a[0] * b[4] + a[2] * b[5] + a[4],   // m4 = a*tx_b + c*ty_b + tx_a
        a[1] * b[4] + a[3] * b[5] + a[5],   // m5
    ]
}

/// Invert a 2D affine. Returns `None` when the linear part is
/// singular (det ≈ 0); callers fall back to identity.
pub fn affine_inverse(m: Affine) -> Option<Affine> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    // Inverse of the linear 2x2:
    //   | a c |^-1   1   |  d -c |
    //   | b d |    = --- | -b  a |
    //                det
    let ia = m[3] * inv;
    let ib = -m[1] * inv;
    let ic = -m[2] * inv;
    let id = m[0] * inv;
    // Translation: inverse * (-t)
    let itx = -(ia * m[4] + ic * m[5]);
    let ity = -(ib * m[4] + id * m[5]);
    Some([ia, ib, ic, id, itx, ity])
}

/// Track L — rebase delta for a group transform change from `G_old`
/// to `G_new`. The delta is `G_new * inv(G_old)`; applied to a leaf's
/// pre-baked transform (`leaf' = delta * leaf`) it produces the
/// new leaf transform such that the rendered output equals
/// `G_new * leaf_local` for every leaf. `None` on `G_old` is
/// treated as identity.
pub fn group_rebase_delta(g_old: Option<Affine>, g_new: Affine) -> Option<Affine> {
    let g_old = g_old.unwrap_or(AFFINE_IDENTITY);
    let inv_old = affine_inverse(g_old)?;
    Some(affine_multiply(g_new, inv_old))
}

#[cfg(test)]
// 0.7071 literals are cos/sin 45° rotation fixtures; the explicit affine
// matrix reads clearer than FRAC_1_SQRT_2 here.
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;

    fn close(a: P, b: P, eps: f32) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps
    }

    #[test]
    fn split_at_half_lies_on_the_curve() {
        // Straight horizontal segment from (0,0) to (10,0) with
        // collinear handles — t=0.5 should land exactly at (5,0).
        let s = split_segment_de_casteljau([0.0, 0.0], [3.0, 0.0], [7.0, 0.0], [10.0, 0.0], 0.5);
        assert!(close(s.mid_anchor, [5.0, 0.0], 1e-5));
    }

    #[test]
    fn split_preserves_curve_endpoints_when_t_extreme() {
        // t=0 → mid_anchor coincides with start; t=1 → with end.
        let start = [0.0_f32, 0.0_f32];
        let end = [10.0_f32, 0.0_f32];
        let s0 = split_segment_de_casteljau(start, [3.0, 0.0], [7.0, 0.0], end, 0.0);
        let s1 = split_segment_de_casteljau(start, [3.0, 0.0], [7.0, 0.0], end, 1.0);
        assert!(close(s0.mid_anchor, start, 1e-5));
        assert!(close(s1.mid_anchor, end, 1e-5));
    }

    #[test]
    fn split_then_evaluate_preserves_shape() {
        // Curved segment: start=(0,0), end=(10,10), handles bowed.
        // Compute the mid-anchor via split at t=0.4, then verify
        // the original cubic evaluated at t=0.4 matches.
        let start = [0.0_f32, 0.0_f32];
        let h0 = [4.0_f32, 0.0_f32];
        let h1 = [6.0_f32, 10.0_f32];
        let end = [10.0_f32, 10.0_f32];
        let t = 0.4_f32;
        let split = split_segment_de_casteljau(start, h0, h1, end, t);

        // Direct cubic evaluation: B(t) = (1-t)^3 P0 + 3(1-t)^2 t P1
        //                                + 3(1-t) t^2 P2 + t^3 P3
        let inv = 1.0 - t;
        let direct = [
            inv.powi(3) * start[0]
                + 3.0 * inv.powi(2) * t * h0[0]
                + 3.0 * inv * t.powi(2) * h1[0]
                + t.powi(3) * end[0],
            inv.powi(3) * start[1]
                + 3.0 * inv.powi(2) * t * h0[1]
                + 3.0 * inv * t.powi(2) * h1[1]
                + t.powi(3) * end[1],
        ];
        assert!(close(split.mid_anchor, direct, 1e-4));
    }

    #[test]
    fn smooth_handles_horizontal_neighbours() {
        // prev = (-3, 0), curr = (0, 0), next = (6, 0):
        // tangent along +x, prev_dist=3, next_dist=6.
        // left = curr - (1/3)*3*x = (-1, 0)
        // right = curr + (1/3)*6*x = (2, 0)
        let (l, r) = smooth_handles_from_neighbours([-3.0, 0.0], [0.0, 0.0], [6.0, 0.0]);
        assert!(close(l, [-1.0, 0.0], 1e-5));
        assert!(close(r, [2.0, 0.0], 1e-5));
    }

    #[test]
    fn smooth_handles_degenerate_prev_equals_next() {
        // No tangent direction → corner fallback.
        let (l, r) = smooth_handles_from_neighbours([1.0, 2.0], [5.0, 6.0], [1.0, 2.0]);
        assert_eq!(l, [5.0, 6.0]);
        assert_eq!(r, [5.0, 6.0]);
    }

    // ---- Track L — affine algebra ----------------------------------------

    fn close6(a: Affine, b: Affine, eps: f32) -> bool {
        (0..6).all(|i| (a[i] - b[i]).abs() < eps)
    }

    #[test]
    fn affine_multiply_identity_is_noop() {
        let m = [1.5, 0.0, 0.0, 2.0, 5.0, -3.0];
        assert!(close6(affine_multiply(AFFINE_IDENTITY, m), m, 1e-5));
        assert!(close6(affine_multiply(m, AFFINE_IDENTITY), m, 1e-5));
    }

    #[test]
    fn affine_inverse_undoes_multiply() {
        let m = [0.7071, 0.7071, -0.7071, 0.7071, 10.0, 20.0];
        let inv = affine_inverse(m).expect("non-singular");
        let id = affine_multiply(m, inv);
        assert!(close6(id, AFFINE_IDENTITY, 1e-4));
    }

    #[test]
    fn affine_inverse_singular_returns_none() {
        // 2x2 has det 0 → uninvertible.
        let m = [1.0, 2.0, 2.0, 4.0, 0.0, 0.0];
        assert!(affine_inverse(m).is_none());
    }

    #[test]
    fn group_rebase_round_trips_leaf_through_g_old_to_g_new_and_back() {
        // Track L invariant: if a leaf's effective transform was
        // `M_leaf = G_old * L_local`, then after applying the
        // group rebase delta the new leaf becomes
        // `G_new * inv(G_old) * M_leaf`. That equals
        // `G_new * inv(G_old) * G_old * L_local = G_new * L_local`,
        // so the leaf still lives in the local coords of the
        // (now-rotated) group.
        let g_old = [0.7071, 0.7071, -0.7071, 0.7071, 100.0, 50.0];
        let g_new = [0.5, 0.866, -0.866, 0.5, 200.0, -10.0];
        // Leaf local position inside the group.
        let l_local: Affine = [1.0, 0.0, 0.0, 1.0, 30.0, 40.0];
        let m_leaf = affine_multiply(g_old, l_local);
        let delta = group_rebase_delta(Some(g_old), g_new).expect("invertible");
        let m_leaf_new = affine_multiply(delta, m_leaf);
        let expected = affine_multiply(g_new, l_local);
        assert!(close6(m_leaf_new, expected, 1e-3));
    }

    #[test]
    fn group_rebase_handles_none_old_as_identity() {
        let g_new = [0.5, 0.866, -0.866, 0.5, 200.0, -10.0];
        let delta = group_rebase_delta(None, g_new).expect("identity invertible");
        // delta == g_new * inv(I) == g_new.
        assert!(close6(delta, g_new, 1e-5));
    }

    #[test]
    fn fit_straight_line_keeps_endpoints_on_the_line() {
        let pts: Vec<PathAnchorSpec> = (0..10)
            .map(|i| {
                let x = i as f32 * 10.0;
                PathAnchorSpec {
                    anchor: [x, 0.0],
                    left: [x, 0.0],
                    right: [x, 0.0],
                }
            })
            .collect();
        let fitted = fit_polyline_to_anchors(&pts);
        assert!(
            fitted.len() < pts.len(),
            "a straight line fits with few anchors (got {})",
            fitted.len()
        );
        assert_eq!(fitted.first().unwrap().anchor, [0.0, 0.0]);
        assert_eq!(fitted.last().unwrap().anchor, [90.0, 0.0]);
        for a in &fitted {
            assert!(a.anchor[1].abs() < 1e-3, "anchor off the line: {:?}", a.anchor);
            assert!(a.left[1].abs() < 1.0 && a.right[1].abs() < 1.0);
        }
    }

    #[test]
    fn fit_declines_below_three_points() {
        let pts = vec![
            PathAnchorSpec { anchor: [0.0, 0.0], left: [0.0, 0.0], right: [0.0, 0.0] },
            PathAnchorSpec { anchor: [10.0, 5.0], left: [10.0, 5.0], right: [10.0, 5.0] },
        ];
        assert_eq!(fit_polyline_to_anchors(&pts), pts);
    }

    #[test]
    fn anchor_from_split_wires_fields() {
        let s = SegmentSplit {
            start_right: [1.0, 1.0],
            mid_left: [2.0, 2.0],
            mid_anchor: [3.0, 3.0],
            mid_right: [4.0, 4.0],
            end_left: [5.0, 5.0],
        };
        let a = anchor_from_split(s);
        assert_eq!(a.anchor, [3.0, 3.0]);
        assert_eq!(a.left, [2.0, 2.0]);
        assert_eq!(a.right, [4.0, 4.0]);
    }
}
