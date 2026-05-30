//! SDK Phase 5 (v1 sweep) — curve-preserving Bezier CSG.
//!
//! Built on `flo_curves`'s `path_add` / `path_sub` /
//! `path_intersect` / `path_xor` (0.8.0). Operates on idml's
//! `PathAnchor` representation via the conversion helpers in
//! [`crate::bezier_conv`]. Output paths are exact Bezier curves
//! (no polyline flattening) — matches InDesign's Pathfinder
//! behavior for ovals, rounded rects, and other curved shapes.
//!
//! The `accuracy` parameter passed to flo_curves controls how
//! close two curves must be before they're considered
//! intersecting at the subdivision level; 0.01 (one-hundredth of
//! a pt) is well below any visible threshold and keeps subdivision
//! depth bounded.

use flo_curves::bezier::path::{path_add, path_intersect, path_sub, SimpleBezierPath};
use idml_parse::PathAnchor;

use crate::bezier_conv::{flo_to_idml_path, idml_path_to_flo};

/// Pathfinder operation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathfinderKind {
    /// Combine all paths into one (Boolean OR).
    Union,
    /// Keep only the region where all paths overlap (Boolean AND).
    Intersect,
    /// Top minus the union of the rest (top \ ⋃rest).
    Subtract,
    /// Symmetric difference (Boolean XOR).
    Exclude,
}

/// Accuracy used for flo_curves's subdivision-based intersection
/// math. One-hundredth of a point is comfortably below any
/// visible threshold and keeps the recursion bounded.
const PATHFINDER_ACCURACY: f64 = 0.01;

/// Run a Pathfinder boolean over N input paths (each a flat anchor
/// list + subpath_starts). The first input is the "top" path —
/// the only one that's special for Subtract (top minus rest);
/// Union / Intersect / Exclude are symmetric over their inputs.
///
/// Returns the result as `(anchors, subpath_starts)` matching
/// idml's path representation. The result is **closed** — every
/// subpath terminates back at its start point. An empty result
/// (e.g. Intersect of two non-overlapping paths) returns
/// `(vec![], vec![])`.
pub fn pathfinder_boolean(
    inputs: &[(Vec<PathAnchor>, Vec<usize>)],
    kind: PathfinderKind,
) -> (Vec<PathAnchor>, Vec<usize>) {
    if inputs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let flo_inputs: Vec<Vec<SimpleBezierPath>> = inputs
        .iter()
        .map(|(anchors, starts)| idml_path_to_flo(anchors, starts))
        .collect();
    if flo_inputs.iter().all(|p| p.is_empty()) {
        return (Vec::new(), Vec::new());
    }
    let result: Vec<SimpleBezierPath> = match kind {
        PathfinderKind::Union => {
            let mut acc: Vec<SimpleBezierPath> = flo_inputs[0].clone();
            for other in &flo_inputs[1..] {
                acc = path_add::<SimpleBezierPath>(
                    &acc,
                    other,
                    PATHFINDER_ACCURACY,
                );
            }
            acc
        }
        PathfinderKind::Intersect => {
            let mut acc: Vec<SimpleBezierPath> = flo_inputs[0].clone();
            for other in &flo_inputs[1..] {
                acc = path_intersect::<SimpleBezierPath>(
                    &acc,
                    other,
                    PATHFINDER_ACCURACY,
                );
            }
            acc
        }
        PathfinderKind::Subtract => {
            // top \ ⋃(rest). Collapse `rest` into a union path
            // first so a single `path_sub` call handles the full
            // subtraction.
            let mut rest: Vec<SimpleBezierPath> = Vec::new();
            for other in &flo_inputs[1..] {
                if rest.is_empty() {
                    rest = other.clone();
                } else {
                    rest = path_add::<SimpleBezierPath>(
                        &rest,
                        other,
                        PATHFINDER_ACCURACY,
                    );
                }
            }
            if rest.is_empty() {
                flo_inputs[0].clone()
            } else {
                path_sub::<SimpleBezierPath>(
                    &flo_inputs[0],
                    &rest,
                    PATHFINDER_ACCURACY,
                )
            }
        }
        PathfinderKind::Exclude => {
            // XOR ≡ (A ∪ B) − (A ∩ B). flo_curves 0.8 doesn't
            // expose a direct path_xor for SimpleBezierPath, so
            // compose it from union + intersect + subtract. For
            // ≥3 inputs the result is `((A ∪ B ∪ ...) − (A ∩ B
            // ∩ ...))` — that's the n-ary generalization
            // InDesign uses.
            let mut union_acc: Vec<SimpleBezierPath> = flo_inputs[0].clone();
            for other in &flo_inputs[1..] {
                union_acc = path_add::<SimpleBezierPath>(
                    &union_acc,
                    other,
                    PATHFINDER_ACCURACY,
                );
            }
            let mut isect_acc: Vec<SimpleBezierPath> = flo_inputs[0].clone();
            for other in &flo_inputs[1..] {
                isect_acc = path_intersect::<SimpleBezierPath>(
                    &isect_acc,
                    other,
                    PATHFINDER_ACCURACY,
                );
            }
            if isect_acc.is_empty() {
                union_acc
            } else {
                path_sub::<SimpleBezierPath>(
                    &union_acc,
                    &isect_acc,
                    PATHFINDER_ACCURACY,
                )
            }
        }
    };
    flo_to_idml_path(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Vec<PathAnchor> {
        let p = |x: f32, y: f32| (x, y);
        vec![
            PathAnchor {
                anchor: p(left, top),
                left: p(left, top),
                right: p(left, top),
            },
            PathAnchor {
                anchor: p(right, top),
                left: p(right, top),
                right: p(right, top),
            },
            PathAnchor {
                anchor: p(right, bottom),
                left: p(right, bottom),
                right: p(right, bottom),
            },
            PathAnchor {
                anchor: p(left, bottom),
                left: p(left, bottom),
                right: p(left, bottom),
            },
        ]
    }

    #[test]
    fn union_of_two_disjoint_rects_keeps_both() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(20.0, 20.0, 30.0, 30.0);
        let (anchors, starts) = pathfinder_boolean(
            &[(a, vec![]), (b, vec![])],
            PathfinderKind::Union,
        );
        // Two subpaths: one per disjoint rect.
        assert_eq!(starts.len(), 2);
        // 4 corners per rect = 8 anchors total.
        assert_eq!(anchors.len(), 8);
    }

    #[test]
    fn intersect_of_non_overlapping_rects_is_empty() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(20.0, 20.0, 30.0, 30.0);
        let (anchors, _) = pathfinder_boolean(
            &[(a, vec![]), (b, vec![])],
            PathfinderKind::Intersect,
        );
        assert!(anchors.is_empty());
    }

    #[test]
    fn intersect_of_overlapping_rects_is_overlap() {
        // A = [0..20, 0..20], B = [10..30, 10..30] → intersect = [10..20, 10..20].
        let a = rect(0.0, 0.0, 20.0, 20.0);
        let b = rect(10.0, 10.0, 30.0, 30.0);
        let (anchors, starts) = pathfinder_boolean(
            &[(a, vec![]), (b, vec![])],
            PathfinderKind::Intersect,
        );
        assert_eq!(starts.len(), 1);
        assert_eq!(anchors.len(), 4);
        let xs: Vec<f32> = anchors.iter().map(|a| a.anchor.0).collect();
        let ys: Vec<f32> = anchors.iter().map(|a| a.anchor.1).collect();
        let min_x = xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_x = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min_y = ys.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!((min_x - 10.0).abs() < 1e-3);
        assert!((max_x - 20.0).abs() < 1e-3);
        assert!((min_y - 10.0).abs() < 1e-3);
        assert!((max_y - 20.0).abs() < 1e-3);
    }

    #[test]
    fn subtract_overlapping_punches_hole_or_l_shape() {
        // A = [0..20, 0..20], B = [10..30, 10..30] → A \ B = L-shape.
        let a = rect(0.0, 0.0, 20.0, 20.0);
        let b = rect(10.0, 10.0, 30.0, 30.0);
        let (anchors, starts) = pathfinder_boolean(
            &[(a, vec![]), (b, vec![])],
            PathfinderKind::Subtract,
        );
        assert!(!anchors.is_empty(), "L-shape should be non-empty");
        assert_eq!(starts.len(), 1, "L-shape is one subpath");
        // An L-shape from this subtraction has 6 corner vertices.
        assert_eq!(anchors.len(), 6, "L-shape has 6 corners");
    }

    #[test]
    fn exclude_overlapping_yields_two_l_shapes() {
        // Symmetric difference of two overlapping rects = two
        // L-shapes (one per input minus the overlap).
        let a = rect(0.0, 0.0, 20.0, 20.0);
        let b = rect(10.0, 10.0, 30.0, 30.0);
        let (anchors, starts) = pathfinder_boolean(
            &[(a, vec![]), (b, vec![])],
            PathfinderKind::Exclude,
        );
        assert!(!anchors.is_empty());
        // Two disjoint L-shapes = 2 subpaths.
        assert_eq!(starts.len(), 2);
    }
}
