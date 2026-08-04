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

//! SDK Phase 5 (v1 sweep) — bridge between idml's `PathAnchor`
//! representation and `flo_curves`'s `SimpleBezierPath`.
//!
//! idml's anchor model carries three points per node:
//!   - `anchor`   the on-curve point
//!   - `left`     the incoming Bezier handle (from the previous
//!     segment that ends at this anchor)
//!   - `right`    the outgoing Bezier handle (to the next segment
//!     starting from this anchor)
//!
//! flo_curves's `SimpleBezierPath` is `(Coord2, Vec<(Coord2, Coord2,
//! Coord2)>)` — a start point plus a list of `(cp1, cp2, end)`
//! cubic segments. For a closed path with anchors `[A, B, C]`,
//! the equivalent flo_curves path is:
//!   start = A.anchor
//!   segments = [
//!     (A.right, B.left, B.anchor),
//!     (B.right, C.left, C.anchor),
//!     (C.right, A.left, A.anchor),  -- closing segment back to A
//!   ]
//!
//! Conversion is bytewise round-trippable when the input is a
//! closed polygon. Compound paths with multiple subpaths translate
//! to a `Vec<SimpleBezierPath>` (one entry per subpath); the
//! `subpath_starts` table is preserved.
//!
//! This module is the foundation for any operation that wants
//! curve-level math — Pathfinder is the first user; future
//! candidates are Offset Path, Outline Stroke, and curve
//! simplification.
//!
//! # C-21 — why every path is quantized before a boolean
//!
//! `flo_curves` 0.8.0 ends every boolean — `path_add`, `path_sub`,
//! `path_intersect`, `path_remove_interior_points`,
//! `path_remove_overlapped_points`, and the `GraphPath::exterior_paths`
//! core all of them share — by sorting the arrangement's points with
//! this comparator (`bezier/path/graph_path/mod.rs`):
//!
//! ```text
//! if (x_a - x_b).abs() < 0.01 { cmp by y } else { cmp by x }
//! ```
//!
//! That is not a strict weak ordering. Three points whose x values chain
//! within 0.01 while the outer pair does not are mutually inconsistent
//! (`x = 0.010, 0.005, 0.000` with ascending y is a cycle). Rust's
//! driftsort DETECTS the violation and panics — "user-provided comparison
//! function does not correctly implement a total order" — and the
//! editor's wasm worker builds `panic = abort`, so it is an
//! unrecoverable process abort; `catch_unwind` cannot help where it
//! matters. This is upstream and pre-existing: it is reachable from every
//! boolean the engine ships, not only from the B-22 arrangement.
//!
//! **Upstream's own mitigation does not run.** Each arithmetic entry
//! point calls `merged_path.round(accuracy)` immediately before
//! `exterior_paths`, intending to snap every coordinate onto an
//! `accuracy` grid. But `Coordinate::round` is `fn round(self, f64) ->
//! Self`, and `GraphPath::round` calls it as a bare statement
//! (`self.points[i].position.round(accuracy);`), discarding the result —
//! so it is a **no-op** in 0.8.0 (verified: `round(1.0)` leaves
//! `0.123456` untouched). Raising the `accuracy` argument therefore
//! CANNOT make the comparator safe on its own; measured against a
//! reproduction, `1/64` alone still aborts. 0.8.0 is the newest
//! published release, so there is no version to bump to.
//!
//! So core performs the rounding upstream intended, on its own inputs,
//! before handing them over — that is [`snap_paths_to_grid`]. On the
//! [`BOOLEAN_GRID`] every coordinate is an exact multiple of 2⁻⁶, so two
//! distinct x columns are at least 0.015625 apart — strictly more than
//! the comparator's 0.01 — and the epsilon branch fires only for
//! genuinely equal x. The comparator then collapses to plain
//! lexicographic (x, y), which is transitive by construction rather than
//! by luck.
//!
//! ## Residuals — the hazard is narrowed, not retired
//!
//! - Points that `self_collide` INVENTS (curve/curve crossings) are
//!   interpolated, not grid-aligned. Because 0.015625 < 0.02, one
//!   off-grid crossing can still sit within 0.01 of two adjacent grid
//!   columns and bridge them. Input anchors are the dominant source of
//!   dense x-clusters in real artwork (a boolean carries thousands of
//!   vertices and a handful of crossings), so quantizing removes the
//!   reachable case, not the theoretical one.
//! - `bezier/path/ray.rs`'s `ray_collisions` sorts with the SAME defect
//!   class: `dx.abs() > SMALL_DISTANCE` (0.001) selects between ordering
//!   by ray position and ordering by edge priority, so a chain of
//!   near-coincident collisions is non-transitive too. Quantizing inputs
//!   does NOT fix it — collision positions are interpolated along curves
//!   and are never grid points. It is not currently reachable: it sorts
//!   one ray's collisions (a handful of elements) and Rust's sort only
//!   validates ordering on slices large enough to leave the
//!   insertion-sort path, which is exactly why every observed abort came
//!   from `exterior_paths` instead.
//!
//! The complete fix for both is a vendored/patched `flo_curves` wired in
//! through `[patch.crates-io]` — repair the two comparators, or just make
//! `GraphPath::round` assign its result. Not taken here: the crate is
//! 1.6 MB / 106 files under Apache-2.0, and carrying a vendored copy
//! inside this public MPL-2.0-OR-PMEL repo is a licensing decision for
//! the maintainer, not something a bug fix should decide.

use flo_curves::bezier::path::SimpleBezierPath;
use flo_curves::Coord2;
use paged_model::PathAnchor;

/// The epsilon `flo_curves` 0.8.0 hardcodes in the point sort that
/// `GraphPath::exterior_paths` runs before it walks the exterior. Two
/// points whose x differ by LESS than this are treated as one column and
/// ordered by y; everything else is ordered by x. Recorded here because
/// [`BOOLEAN_GRID`] only works while it stays strictly above this value.
pub const FLO_SORT_EPSILON: f64 = 0.01;

/// C-21 — the coordinate grid every path is snapped onto before it is
/// handed to a `flo_curves` boolean. **Load-bearing for crash-safety, not
/// a quality knob**; see this module's C-21 section for the full
/// derivation. Do not lower it below [`FLO_SORT_EPSILON`].
///
/// `1/64` is the FINEST power of two above 0.01 (`1/128` = 0.0078 is
/// below it). A power of two is what makes the grid exact: `m · 2⁻⁶` is
/// representable in f64 AND in the f32 that [`PathAnchor`] stores, and
/// `(v / g).round() * g` is scaling by a power of two, so no float slop
/// creeps in and distinct grid values differ by exactly ≥ 1/64 at every
/// magnitude. A decimal 0.01 grid — what this code used to pass — does
/// not have that property: over `m ∈ [-200000, 200000]`, 354 909
/// consecutive `0.01` grid pairs compute a difference *below* 0.01,
/// making 0.01 the worst reachable choice.
///
/// Cost: a coordinate moves by at most `1/128 pt` = 0.0078 pt ≈ 0.0028 mm
/// — about a quarter of a device dot at 2400 dpi, and inside the 0.01 pt
/// accuracy these operations already documented.
pub const BOOLEAN_GRID: f64 = 1.0 / 64.0;

// C-21 — the compile-time half of the guard. The grid is worthless the
// moment it drops to or below the epsilon flo_curves hardcodes, and the
// failure mode is a process abort, so a lowered value must break the
// BUILD rather than wait for a test run. The f64-ARITHMETIC half (do
// distinct grid columns really separate by more than the epsilon at
// every reachable magnitude? a nominal 0.01 grid does not) needs a loop
// and lives in `boolean_grid_separates_above_the_flo_curves_sort_epsilon`.
const _: () = assert!(
    BOOLEAN_GRID > FLO_SORT_EPSILON,
    "C-21: BOOLEAN_GRID must stay strictly above flo_curves' 0.01 sort epsilon"
);

/// Snap one coordinate onto a `grid`-sized lattice.
fn snap_coord(c: Coord2, grid: f64) -> Coord2 {
    Coord2((c.0 / grid).round() * grid, (c.1 / grid).round() * grid)
}

/// C-21 — quantize every coordinate (anchors AND control points) of
/// `paths` onto a `grid`-sized lattice, in place.
///
/// Call this on anything about to enter a `flo_curves` path function.
/// With `grid` = [`BOOLEAN_GRID`] it is what keeps `exterior_paths`'
/// point sort transitive; see the C-21 section at the top of this module
/// for why the `accuracy` argument alone cannot do it.
pub fn snap_paths_to_grid(paths: &mut [SimpleBezierPath], grid: f64) {
    for (start, segments) in paths.iter_mut() {
        *start = snap_coord(*start, grid);
        for (cp1, cp2, end) in segments.iter_mut() {
            *cp1 = snap_coord(*cp1, grid);
            *cp2 = snap_coord(*cp2, grid);
            *end = snap_coord(*end, grid);
        }
    }
}

/// Convert one idml subpath (slice of contiguous PathAnchors that
/// form a single contour) to a flo_curves `SimpleBezierPath`. The
/// subpath is treated as **closed** — a final segment from the
/// last anchor back to the first is emitted using the last
/// anchor's `right` handle and the first anchor's `left` handle.
///
/// Returns `None` for empty subpaths (no anchors to convert).
pub fn idml_subpath_to_flo(anchors: &[PathAnchor]) -> Option<SimpleBezierPath> {
    if anchors.is_empty() {
        return None;
    }
    let first = &anchors[0];
    let start = Coord2(first.anchor.0 as f64, first.anchor.1 as f64);
    let mut segments: Vec<(Coord2, Coord2, Coord2)> = Vec::with_capacity(anchors.len());
    for i in 0..anchors.len() {
        let from = &anchors[i];
        let to = &anchors[(i + 1) % anchors.len()];
        let cp1 = Coord2(from.right.0 as f64, from.right.1 as f64);
        let cp2 = Coord2(to.left.0 as f64, to.left.1 as f64);
        let end = Coord2(to.anchor.0 as f64, to.anchor.1 as f64);
        segments.push((cp1, cp2, end));
    }
    Some((start, segments))
}

/// Convert a full idml path (flat anchor list + subpath_starts) to
/// the list of `SimpleBezierPath`s that flo_curves's path
/// arithmetic expects. An empty `subpath_starts` is treated as a
/// single subpath covering every anchor.
pub fn idml_path_to_flo(anchors: &[PathAnchor], subpath_starts: &[usize]) -> Vec<SimpleBezierPath> {
    if anchors.is_empty() {
        return Vec::new();
    }
    if subpath_starts.is_empty() {
        return idml_subpath_to_flo(anchors).into_iter().collect();
    }
    let mut out = Vec::with_capacity(subpath_starts.len());
    for i in 0..subpath_starts.len() {
        let start = subpath_starts[i];
        let end = if i + 1 < subpath_starts.len() {
            subpath_starts[i + 1]
        } else {
            anchors.len()
        };
        if start >= anchors.len() || end > anchors.len() || start >= end {
            continue;
        }
        if let Some(p) = idml_subpath_to_flo(&anchors[start..end]) {
            out.push(p);
        }
    }
    out
}

/// [`idml_path_to_flo`] followed by [`snap_paths_to_grid`] — the door
/// every boolean call site should use. Keeping the two steps married in
/// one function is deliberate: an unsnapped path reaching
/// `exterior_paths` is a process abort, not a quality regression (C-21).
pub fn idml_path_to_flo_on_grid(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    grid: f64,
) -> Vec<SimpleBezierPath> {
    let mut paths = idml_path_to_flo(anchors, subpath_starts);
    snap_paths_to_grid(&mut paths, grid);
    paths
}

/// Convert a list of flo_curves `SimpleBezierPath`s back to an
/// idml-style flat anchor list + `subpath_starts`. Each input path
/// becomes one closed subpath in the output. The conversion is
/// lossless for paths produced by flo_curves's boolean ops (the
/// closing segment lines up with the start point bytewise).
///
/// Anchor layout per path: for N segments, emit N anchors. The
/// k-th anchor sits at segment-k's `end` (or the path's `start`
/// when k = 0). Its `left` handle is segment-k's `cp2`; its
/// `right` handle is segment-(k+1)'s `cp1` (wrapping around for
/// the last anchor).
pub fn flo_to_idml_path(paths: &[SimpleBezierPath]) -> (Vec<PathAnchor>, Vec<usize>) {
    let mut anchors: Vec<PathAnchor> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for (start_point, segments) in paths {
        if segments.is_empty() {
            continue;
        }
        starts.push(anchors.len());
        // Anchor 0 sits at the path's start point. Its `left` is
        // the last segment's cp2 (incoming on the closing
        // segment); its `right` is segment[0]'s cp1.
        let last = &segments[segments.len() - 1];
        anchors.push(PathAnchor {
            anchor: (start_point.0 as f32, start_point.1 as f32),
            left: (last.1 .0 as f32, last.1 .1 as f32),
            right: (segments[0].0 .0 as f32, segments[0].0 .1 as f32),
        });
        // Anchors 1..N sit at each segment's end. Skip the final
        // segment (its end is the path's start point, already
        // captured as anchor 0). For each interior segment i,
        // anchor.left = segment[i].cp2, anchor.right =
        // segment[i+1].cp1.
        for i in 0..(segments.len().saturating_sub(1)) {
            let seg = &segments[i];
            let next_seg = &segments[i + 1];
            anchors.push(PathAnchor {
                anchor: (seg.2 .0 as f32, seg.2 .1 as f32),
                left: (seg.1 .0 as f32, seg.1 .1 as f32),
                right: (next_seg.0 .0 as f32, next_seg.0 .1 as f32),
            });
        }
    }
    (anchors, starts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_anchors(left: f32, top: f32, right: f32, bottom: f32) -> Vec<PathAnchor> {
        // Corner-only rectangle: handles equal the anchor (no curve).
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
    fn rect_round_trips_through_flo() {
        let anchors = rect_anchors(0.0, 0.0, 100.0, 50.0);
        let flo = idml_path_to_flo(&anchors, &[]);
        assert_eq!(flo.len(), 1);
        let (start, segs) = &flo[0];
        assert_eq!((start.0 as f32, start.1 as f32), (0.0, 0.0));
        assert_eq!(segs.len(), 4); // 4 sides
        let (back_anchors, back_starts) = flo_to_idml_path(&flo);
        assert_eq!(back_starts, vec![0]);
        assert_eq!(back_anchors.len(), 4);
        for (orig, back) in anchors.iter().zip(back_anchors.iter()) {
            assert!((orig.anchor.0 - back.anchor.0).abs() < 1e-3);
            assert!((orig.anchor.1 - back.anchor.1).abs() < 1e-3);
        }
    }

    /// C-21 GUARD — do not lower [`BOOLEAN_GRID`].
    ///
    /// The grid is what keeps `flo_curves`' `exterior_paths` point sort a
    /// strict weak ordering, and a violated ordering is a PANIC (an abort
    /// in the wasm worker), not a wrong pixel. Two properties have to
    /// hold: the grid step must exceed the comparator's hardcoded
    /// epsilon, and it must exceed it in f64 ARITHMETIC — a nominal
    /// 0.01 grid fails the second test at ordinary page coordinates,
    /// which is what made the previous value the worst possible one.
    #[test]
    fn boolean_grid_separates_above_the_flo_curves_sort_epsilon() {
        // `grid > FLO_SORT_EPSILON` itself is asserted at COMPILE time
        // next to the constant; the m = 0 step of the loop below re-checks
        // it, and the rest of the loop checks the part a const assert
        // cannot: that the separation survives f64 arithmetic.

        // Power of two: exact in f64 and in the f32 PathAnchor stores.
        assert_eq!(
            BOOLEAN_GRID.log2(),
            BOOLEAN_GRID.log2().round(),
            "grid must be a power of two"
        );
        // ±200000 steps of the grid covers ±3125 pt — far beyond any page
        // coordinate, and the property is scale-free above that.
        for m in -200_000_i64..=200_000 {
            let lo = (m as f64) * BOOLEAN_GRID;
            let hi = ((m + 1) as f64) * BOOLEAN_GRID;
            assert!(
                hi - lo > FLO_SORT_EPSILON,
                "grid step collapses under f64 at m={m}: {lo} → {hi}"
            );
            // Snapping an on-grid value must be a fixed point, or the
            // "distinct columns are ≥ one step apart" argument leaks.
            assert_eq!(
                snap_coord(Coord2(lo, lo), BOOLEAN_GRID),
                Coord2(lo, lo),
                "snap is not idempotent at m={m}"
            );
        }
    }

    #[test]
    fn snapping_lands_every_coordinate_on_the_grid() {
        let anchors = rect_anchors(0.3, -1.7, 10.004, 5.331);
        let raw = idml_path_to_flo(&anchors, &[]);
        let snapped = idml_path_to_flo_on_grid(&anchors, &[], BOOLEAN_GRID);
        let flatten = |paths: &[SimpleBezierPath]| -> Vec<Coord2> {
            paths
                .iter()
                .flat_map(|(start, segments)| {
                    std::iter::once(*start)
                        .chain(segments.iter().flat_map(|(a, b, e)| [*a, *b, *e]))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let (raw, snapped) = (flatten(&raw), flatten(&snapped));
        assert_eq!(raw.len(), snapped.len());
        for (before, after) in raw.iter().zip(snapped.iter()) {
            assert_eq!(
                snap_coord(*after, BOOLEAN_GRID),
                *after,
                "off-grid {after:?}"
            );
            // No coordinate moves by more than half a grid step.
            assert!((after.0 - before.0).abs() <= BOOLEAN_GRID / 2.0);
            assert!((after.1 - before.1).abs() <= BOOLEAN_GRID / 2.0);
        }
    }

    #[test]
    fn compound_path_preserves_subpath_starts() {
        // Two rectangles in one anchor list, marked by subpath_starts.
        let mut anchors = rect_anchors(0.0, 0.0, 10.0, 10.0);
        anchors.extend(rect_anchors(20.0, 20.0, 30.0, 30.0));
        let starts = vec![0_usize, 4_usize];
        let flo = idml_path_to_flo(&anchors, &starts);
        assert_eq!(flo.len(), 2);
        let (back_anchors, back_starts) = flo_to_idml_path(&flo);
        assert_eq!(back_starts, vec![0, 4]);
        assert_eq!(back_anchors.len(), 8);
    }
}
