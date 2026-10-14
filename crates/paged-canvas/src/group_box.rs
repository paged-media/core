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

//! A group's box, the one InDesign draws around a selected group.
//!
//! A group paints nothing of its own, so its box is derived: the union of
//! its members' outlines, measured in the GROUP's own coordinate space and
//! placed by the group's transform. A rotated group therefore gets a box
//! rotated with it, not the axis-aligned box of its rotated members, and a
//! member rotated inside the group widens the box by its outline's extent
//! along the group's axes.
//!
//! Two model facts shape the computation:
//!
//! - `Group::item_transform` is the RAW `ItemTransform` (relative to the
//!   enclosing group), so the group's placement is its ancestors' chain
//!   composed outer-first, exactly as the parser composes a member's.
//! - every member's `item_transform` is already composed into spread
//!   space, so a member is brought into the group's space by the inverse
//!   of that chain, whatever depth it sits at.
//!
//! Members contribute their OUTLINE (anchors and handles), not the four
//! corners of their bounds: a cubic's extent is solved per segment, so a
//! rotated ellipse adds its true extent rather than its rotated bounding
//! rectangle's.

use paged_model::{Bounds, FrameRef, PathAnchor, Spread};

type Affine = [f32; 6];

const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// The box of the group at `group_idx` in `spread`: bounds in the group's
/// own space and the composed transform that places them in spread space
/// (`None` when no group in the chain carries a transform). `None` for an
/// unknown index or a group with no member geometry.
pub(crate) fn group_box(spread: &Spread, group_idx: usize) -> Option<(Bounds, Option<Affine>)> {
    spread.groups.get(group_idx)?;
    let placement = chain_transform(spread, group_idx);
    // A singular group transform (never in well-formed IDML) has no inner
    // space to measure in; the members' spread-space extent is the honest
    // answer then.
    let (placement, to_group) = match placement {
        Some(m) => match invert(m) {
            Some(inv) => (Some(m), inv),
            None => (None, IDENTITY),
        },
        None => (None, IDENTITY),
    };
    let mut acc: Option<Bounds> = None;
    collect(spread, group_idx, &to_group, &mut acc, 0);
    acc.map(|b| (b, placement))
}

/// The group's raw transform composed with every enclosing group's,
/// outermost first.
fn chain_transform(spread: &Spread, group_idx: usize) -> Option<Affine> {
    let mut acc = spread.groups[group_idx].item_transform;
    let mut current = group_idx;
    // A malformed member list could name an ancestor twice; the depth cap
    // keeps a cycle from spinning.
    for _ in 0..spread.groups.len() {
        let Some(parent) = spread
            .groups
            .iter()
            .position(|g| g.members.contains(&FrameRef::Group(current)))
        else {
            break;
        };
        acc = match (spread.groups[parent].item_transform, acc) {
            (None, a) => a,
            (Some(p), None) => Some(p),
            (Some(p), Some(a)) => Some(multiply(p, a)),
        };
        current = parent;
    }
    acc
}

fn collect(spread: &Spread, gi: usize, to_group: &Affine, acc: &mut Option<Bounds>, depth: usize) {
    if depth > spread.groups.len() {
        return;
    }
    let Some(group) = spread.groups.get(gi) else {
        return;
    };
    for member in &group.members {
        match *member {
            FrameRef::Group(child) => collect(spread, child, to_group, acc, depth + 1),
            leaf => {
                if let Some(b) = leaf_extent(spread, leaf, to_group) {
                    *acc = Some(match acc.take() {
                        Some(a) => union(a, b),
                        None => b,
                    });
                }
            }
        }
    }
}

/// One leaf's outline extent in the group's space.
fn leaf_extent(spread: &Spread, leaf: FrameRef, to_group: &Affine) -> Option<Bounds> {
    let ((anchors, starts), bounds, transform) = match leaf {
        FrameRef::TextFrame(i) => spread.text_frames.get(i).map(|f| {
            let path = (f.anchors.clone(), f.subpath_starts.clone());
            (path, f.bounds, f.item_transform)
        }),
        FrameRef::Rectangle(i) => spread.rectangles.get(i).map(|f| {
            let path = (f.anchors.clone(), f.subpath_starts.clone());
            (path, f.bounds, f.item_transform)
        }),
        // An oval carries no anchor table: its outline is the inscribed
        // four-arc ellipse the renderer draws.
        FrameRef::Oval(i) => spread.ovals.get(i).map(|f| {
            let path = (paged_mutate::apply::oval_anchors(f.bounds), Vec::new());
            (path, f.bounds, f.item_transform)
        }),
        FrameRef::Polygon(i) => spread.polygons.get(i).map(|f| {
            let path = (f.anchors.clone(), f.subpath_starts.clone());
            (path, f.bounds, f.item_transform)
        }),
        FrameRef::GraphicLine(i) => spread.graphic_lines.get(i).map(|f| {
            let path = (f.anchors.clone(), f.subpath_starts.clone());
            (path, f.bounds, f.item_transform)
        }),
        FrameRef::Group(_) => None,
    }?;
    let m = multiply(*to_group, transform.unwrap_or(IDENTITY));
    if anchors.is_empty() {
        // A primitive frame (bounds only) is its bounds rectangle.
        let corners = [
            (bounds.left, bounds.top),
            (bounds.right, bounds.top),
            (bounds.right, bounds.bottom),
            (bounds.left, bounds.bottom),
        ];
        return corners
            .iter()
            .map(|&(x, y)| point_box(apply(&m, x, y)))
            .reduce(union);
    }
    outline_extent(&anchors, &starts, &m)
}

/// The extent of a path (closed subpaths; an open one's closing segment
/// is a straight chord between two of its anchors, so closing it never
/// widens the box) through `m`.
fn outline_extent(anchors: &[PathAnchor], starts: &[usize], m: &Affine) -> Option<Bounds> {
    let mut cuts: Vec<usize> = starts
        .iter()
        .copied()
        .filter(|&s| s < anchors.len())
        .collect();
    if cuts.first() != Some(&0) {
        cuts.insert(0, 0);
    }
    cuts.push(anchors.len());
    let mut acc: Option<Bounds> = None;
    for w in cuts.windows(2) {
        let sub = &anchors[w[0]..w[1]];
        for (i, a) in sub.iter().enumerate() {
            let b = &sub[(i + 1) % sub.len()];
            let seg = cubic_extent(
                apply(m, a.anchor.0, a.anchor.1),
                apply(m, a.right.0, a.right.1),
                apply(m, b.left.0, b.left.1),
                apply(m, b.anchor.0, b.anchor.1),
            );
            acc = Some(match acc {
                Some(x) => union(x, seg),
                None => seg,
            });
        }
    }
    acc
}

/// Exact extent of one cubic Bézier: its end points plus the curve at
/// every root of the derivative, per axis.
fn cubic_extent(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) -> Bounds {
    let mut b = union(point_box(p0), point_box(p3));
    let axis = |v0: f32, v1: f32, v2: f32, v3: f32| -> Vec<f64> {
        let (v0, v1, v2, v3) = (v0 as f64, v1 as f64, v2 as f64, v3 as f64);
        // B'(t)/3 = (a - 2b + c) t² + 2 (b - a) t + a, with a, b, c the
        // control-polygon edge vectors.
        let (ea, eb, ec) = (v1 - v0, v2 - v1, v3 - v2);
        let qa = ea - 2.0 * eb + ec;
        let qb = 2.0 * (eb - ea);
        let qc = ea;
        let mut roots = Vec::new();
        if qa.abs() < 1e-12 {
            if qb.abs() > 1e-12 {
                roots.push(-qc / qb);
            }
        } else {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc >= 0.0 {
                let s = disc.sqrt();
                roots.push((-qb + s) / (2.0 * qa));
                roots.push((-qb - s) / (2.0 * qa));
            }
        }
        roots.retain(|t| *t > 0.0 && *t < 1.0);
        roots
    };
    let eval = |t: f64| -> (f32, f32) {
        let mt = 1.0 - t;
        let (w0, w1, w2, w3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        let x = w0 * p0.0 as f64 + w1 * p1.0 as f64 + w2 * p2.0 as f64 + w3 * p3.0 as f64;
        let y = w0 * p0.1 as f64 + w1 * p1.1 as f64 + w2 * p2.1 as f64 + w3 * p3.1 as f64;
        (x as f32, y as f32)
    };
    for t in axis(p0.0, p1.0, p2.0, p3.0)
        .into_iter()
        .chain(axis(p0.1, p1.1, p2.1, p3.1))
    {
        b = union(b, point_box(eval(t)));
    }
    b
}

fn point_box((x, y): (f32, f32)) -> Bounds {
    Bounds {
        top: y,
        left: x,
        bottom: y,
        right: x,
    }
}

fn union(a: Bounds, b: Bounds) -> Bounds {
    Bounds {
        top: a.top.min(b.top),
        left: a.left.min(b.left),
        bottom: a.bottom.max(b.bottom),
        right: a.right.max(b.right),
    }
}

fn apply(m: &Affine, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// `a ∘ b`: `b` applies first.
fn multiply(a: Affine, b: Affine) -> Affine {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

fn invert(m: Affine) -> Option<Affine> {
    crate::hit::invert_affine(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_straight_segment_is_its_end_points() {
        let b = cubic_extent((0.0, 0.0), (0.0, 0.0), (10.0, 5.0), (10.0, 5.0));
        assert_eq!((b.left, b.top, b.right, b.bottom), (0.0, 0.0, 10.0, 5.0));
    }

    #[test]
    fn a_bulging_segment_reaches_its_extremum() {
        // Symmetric arch from (0,0) to (10,0) with both handles at y = -8:
        // the apex sits at t = 0.5, y = -6.
        let b = cubic_extent((0.0, 0.0), (0.0, -8.0), (10.0, -8.0), (10.0, 0.0));
        assert!((b.top + 6.0).abs() < 1e-5, "top {}", b.top);
        assert_eq!((b.left, b.right, b.bottom), (0.0, 10.0, 0.0));
    }

    #[test]
    fn a_rotated_ellipse_adds_its_true_extent() {
        // A 100 × 50 ellipse rotated 45°: half extents are
        // sqrt((50² + 25²) / 2) ≈ 39.53 on both axes, where the rotated
        // bounding rectangle would claim (50 + 25) / √2 ≈ 53.03.
        let anchors = paged_mutate::apply::oval_anchors(Bounds {
            top: -25.0,
            left: -50.0,
            bottom: 25.0,
            right: 50.0,
        });
        let (s, c) = std::f32::consts::FRAC_PI_4.sin_cos();
        let b = outline_extent(&anchors, &[], &[c, s, -s, c, 0.0, 0.0]).unwrap();
        let want = ((50.0f32 * 50.0 + 25.0 * 25.0) / 2.0).sqrt();
        // The four-arc ellipse is the standard kappa approximation, within
        // 0.03 % of the true ellipse.
        for v in [b.right, -b.left, b.bottom, -b.top] {
            assert!((v - want).abs() < 0.05, "{v} vs {want}");
        }
    }
}
