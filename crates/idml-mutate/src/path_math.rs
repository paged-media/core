//! Pure geometry helpers for Track J path-topology ops.
//!
//! Two operations:
//!   - `split_segment_de_casteljau` — inserts a new anchor on a
//!     cubic Bezier segment at parameter `t` without altering the
//!     curve's visible shape. Returns the new mid-anchor + the four
//!     adjusted handles (two on the neighbours, two on the new
//!     anchor itself).
//!   - `smooth_handles_from_neighbours` — derives a smooth (left,
//!     right) pair for an anchor from its previous + next anchor
//!     positions, using the standard 1/3-distance heuristic. Used by
//!     the corner→smooth toggle.
//!
//! All math runs in the path's local coordinate system. No clamping,
//! no document state — the apply layer composes these with index
//! bookkeeping (`subpath_starts`) and `PathAnchorSpec` capture.

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

#[cfg(test)]
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
