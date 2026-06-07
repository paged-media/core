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

use flo_curves::bezier::path::SimpleBezierPath;
use flo_curves::Coord2;
use paged_parse::PathAnchor;

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
