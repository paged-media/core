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

//! Phase E — snapping infrastructure.
//!
//! `compute_snap_adjustment` runs in `update_gesture` (and again at
//! `commit_gesture` since the session stores the snap-adjusted delta).
//! Given a translate gesture's raw pointer delta, it returns a
//! delta-adjusted-to-snap plus the active snap lines for the overlay
//! to render.
//!
//! Snap targets, in priority order:
//!   1. Page edges (left, right, hcenter, top, bottom, vcenter) on the
//!      page hosting the moving items.
//!   2. Sibling frame edges + centres on the same page.
//!
//! Tolerance is in document-space pt. A true industry-strength snap
//! converts the tolerance through the camera to keep it constant in
//! screen px regardless of zoom — that's a Phase E v2 follow-up.
//!
//! Snap currently activates for `GestureType::Translate` on un-rotated
//! members only. Resize / Rotate / Scale snapping arrives later.

use paged_renderer::PageId;
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::gesture::GestureSession;

/// Axis the snap line guides. `X` is a vertical guide (snaps the x
/// coordinate); `Y` is a horizontal guide (snaps the y coordinate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum SnapAxis {
    X,
    Y,
}

/// One active snap line surfaced to the overlay. `position` is in
/// page-local pt on `page_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapLine {
    pub axis: SnapAxis,
    pub position: f32,
    pub page_id: PageId,
}

/// Snap-adjusted delta + the active guides. Returned by
/// `compute_snap_adjustment`.
#[derive(Debug, Default, Clone)]
pub struct SnapAdjustment {
    pub delta: (f32, f32),
    pub lines: Vec<SnapLine>,
}

/// Snap tolerance expressed in CSS px. Converted to doc-space pt at
/// snap time via `camera_scale` from the active gesture session, so
/// the snap behaves naturally regardless of zoom (Phase G).
pub const SNAP_TOLERANCE_CSS_PX: f32 = 4.0;

/// Legacy alias — same value, kept for tests that assert against a
/// doc-space tolerance at camera scale = 1.
pub const SNAP_TOLERANCE_PT: f32 = SNAP_TOLERANCE_CSS_PX;

/// Compute the snap-adjusted delta + active guides for a Translate
/// gesture. Phase H — multi-page aware: each moving frame snaps
/// against its OWN host page's targets; the single shared delta
/// picks the smallest-magnitude adjustment across every
/// (member, target) pair. Pass-through (delta unchanged, lines empty)
/// for any other gesture type or for any rotated member.
pub(crate) fn compute_snap_adjustment(
    session: &GestureSession,
    raw_delta: (f32, f32),
    pages: &[PageInfo],
    siblings: &[FrameRect],
) -> SnapAdjustment {
    use crate::gesture::GestureType;
    if !matches!(session.gesture, GestureType::Translate) {
        return SnapAdjustment {
            delta: raw_delta,
            lines: Vec::new(),
        };
    }
    // Plan-2 §8.4 — Ctrl bypass. When the caller asks to skip snap,
    // pass the raw delta through and surface no snap lines.
    if session.modifiers.disable_snap {
        return SnapAdjustment {
            delta: raw_delta,
            lines: Vec::new(),
        };
    }
    for snap in &session.snapshots {
        if !is_pure_translate_or_identity(snap.item_transform) {
            return SnapAdjustment {
                delta: raw_delta,
                lines: Vec::new(),
            };
        }
    }
    let scale = session.camera_scale.unwrap_or(1.0).max(1e-3);
    let tolerance = SNAP_TOLERANCE_CSS_PX / scale;
    let (dx, dy) = raw_delta;

    // Per-member candidates × per-member targets. Each snap line is
    // tagged with its true host page so the overlay draws it on the
    // right page even in a multi-page selection.
    let mut best_x: Option<SnapMatch> = None;
    let mut best_y: Option<SnapMatch> = None;
    for snap in &session.snapshots {
        let Some(page) = host_page_for_snapshot(snap, pages) else {
            continue;
        };
        let Some(aabb) = snapshot_aabb_in_page(snap, &page) else {
            continue;
        };
        let cand_x = [
            aabb.left + dx,
            (aabb.left + aabb.right) * 0.5 + dx,
            aabb.right + dx,
        ];
        let cand_y = [
            aabb.top + dy,
            (aabb.top + aabb.bottom) * 0.5 + dy,
            aabb.bottom + dy,
        ];
        let targets_x = snap_targets_x(&page, siblings, &session.snapshots);
        let targets_y = snap_targets_y(&page, siblings, &session.snapshots);
        if let Some((adj, target)) = snap_axis_match(&cand_x, &targets_x, tolerance) {
            if best_x
                .as_ref()
                .map_or(true, |b| adj.abs() < b.adjustment.abs())
            {
                best_x = Some(SnapMatch {
                    adjustment: adj,
                    target,
                    page_id: page.page_id.clone(),
                });
            }
        }
        if let Some((adj, target)) = snap_axis_match(&cand_y, &targets_y, tolerance) {
            if best_y
                .as_ref()
                .map_or(true, |b| adj.abs() < b.adjustment.abs())
            {
                best_y = Some(SnapMatch {
                    adjustment: adj,
                    target,
                    page_id: page.page_id.clone(),
                });
            }
        }
    }

    let adj_dx = best_x.as_ref().map(|b| b.adjustment).unwrap_or(0.0);
    let adj_dy = best_y.as_ref().map(|b| b.adjustment).unwrap_or(0.0);
    let mut lines = Vec::new();

    // Plan-2 §8.2 — Smart guides. The snap winner is one alignment;
    // surface every OTHER alignment that's also exactly true after
    // the chosen adjustment so the user sees co-aligned edges they're
    // already on. Pure visual hint — does not affect the delta. We
    // walk each member's candidates against every target and emit a
    // line wherever the post-adjusted candidate hits the target
    // within `SMART_GUIDE_EPSILON_PT` (sub-pixel noise tolerance).
    let post_dx = dx + adj_dx;
    let post_dy = dy + adj_dy;
    for snap in &session.snapshots {
        let Some(page) = host_page_for_snapshot(snap, pages) else {
            continue;
        };
        let Some(aabb) = snapshot_aabb_in_page(snap, &page) else {
            continue;
        };
        let cand_x = [
            aabb.left + post_dx,
            (aabb.left + aabb.right) * 0.5 + post_dx,
            aabb.right + post_dx,
        ];
        let cand_y = [
            aabb.top + post_dy,
            (aabb.top + aabb.bottom) * 0.5 + post_dy,
            aabb.bottom + post_dy,
        ];
        let targets_x = snap_targets_x(&page, siblings, &session.snapshots);
        let targets_y = snap_targets_y(&page, siblings, &session.snapshots);
        for &cand in &cand_x {
            for &target in &targets_x {
                if (target - cand).abs() <= SMART_GUIDE_EPSILON_PT {
                    push_unique_line(
                        &mut lines,
                        SnapLine {
                            axis: SnapAxis::X,
                            position: target,
                            page_id: page.page_id.clone(),
                        },
                    );
                }
            }
        }
        for &cand in &cand_y {
            for &target in &targets_y {
                if (target - cand).abs() <= SMART_GUIDE_EPSILON_PT {
                    push_unique_line(
                        &mut lines,
                        SnapLine {
                            axis: SnapAxis::Y,
                            position: target,
                            page_id: page.page_id.clone(),
                        },
                    );
                }
            }
        }
    }

    // The winning snap line is already inside `lines` via the
    // smart-guide pass (its candidate aligns with its target by
    // construction). Keep the winner-first invariant the overlay
    // historically relied on by ensuring it comes first in its axis.
    promote_winner(&mut lines, best_x.as_ref(), SnapAxis::X);
    promote_winner(&mut lines, best_y.as_ref(), SnapAxis::Y);

    SnapAdjustment {
        delta: (dx + adj_dx, dy + adj_dy),
        lines,
    }
}

/// Tolerance (pt) within which a candidate edge counts as "exactly
/// aligned" with a target for smart-guide rendering. Tighter than the
/// snap tolerance (~4 pt) because smart guides describe an alignment
/// the post-snap frame is ON, not one it would BE PULLED INTO. Float
/// noise from accumulated transform math is the dominant signal.
const SMART_GUIDE_EPSILON_PT: f32 = 0.5;

/// Append `line` only when no existing entry covers the same
/// (axis, position-on-the-same-page) — multiple moving frames often
/// align with the same target and we don't want duplicate overlay
/// chrome. Position equality uses sub-pixel tolerance.
fn push_unique_line(lines: &mut Vec<SnapLine>, line: SnapLine) {
    if lines.iter().any(|l| {
        l.axis == line.axis
            && l.page_id == line.page_id
            && (l.position - line.position).abs() < 1e-3
    }) {
        return;
    }
    lines.push(line);
}

/// Reorder so the winner-axis snap line lands at the front of the
/// axis's lines. Stable behaviour for callers (overlay, K.3 spec)
/// that historically read `lines[0]` / `lines.find(axis == X)` as
/// "the snap winner."
fn promote_winner(lines: &mut Vec<SnapLine>, winner: Option<&SnapMatch>, axis: SnapAxis) {
    let Some(m) = winner else { return };
    let Some(pos) = lines.iter().position(|l| {
        l.axis == axis && l.page_id == m.page_id && (l.position - m.target).abs() < 1e-3
    }) else {
        return;
    };
    // Find the FIRST index in `lines` whose axis matches; move our
    // winner there. Stable; preserves relative order of other lines.
    let Some(first_for_axis) = lines.iter().position(|l| l.axis == axis) else {
        return;
    };
    if pos == first_for_axis {
        return;
    }
    let winner_line = lines.remove(pos);
    lines.insert(first_for_axis, winner_line);
}

#[derive(Debug, Clone)]
struct SnapMatch {
    adjustment: f32,
    target: f32,
    page_id: PageId,
}

/// Page summary fed into the snap pass.
#[derive(Debug, Clone, Default)]
pub(crate) struct PageInfo {
    pub page_id: PageId,
    pub width_pt: f32,
    pub height_pt: f32,
    /// Page origin in spread coords (matches `BuiltPage::spread_origin`).
    pub spread_origin: (f32, f32),
    /// Plan-2 §8.3 — ruler guides on this page. Vertical guide
    /// locations (x in page-local pt) and horizontal guide locations
    /// (y in page-local pt) are pre-split so the snap target builders
    /// can append them without re-discriminating per call. Empty for
    /// pages with no `<Guide>` declarations.
    pub vertical_guides: Vec<f32>,
    pub horizontal_guides: Vec<f32>,
}

/// Snapshot of one frame's geometry used for snap targeting. Carries
/// the frame's AABB in PAGE-LOCAL pt + its identity so the moving
/// items are excluded from their own snap targets.
#[derive(Debug, Clone)]
pub(crate) struct FrameRect {
    pub element_id: crate::element_selection::ElementId,
    pub page_id: PageId,
    /// `[top, left, bottom, right]` in page-local pt.
    pub aabb: [f32; 4],
}

fn is_pure_translate_or_identity(m: Option<[f32; 6]>) -> bool {
    match m {
        None => true,
        Some([a, b, c, d, _, _]) => {
            (a - 1.0).abs() < 1e-4 && (d - 1.0).abs() < 1e-4 && b.abs() < 1e-4 && c.abs() < 1e-4
        }
    }
}

fn host_page_for_snapshot(
    snap: &crate::gesture::NodeSnapshot,
    pages: &[PageInfo],
) -> Option<PageInfo> {
    let aabb = snapshot_aabb_in_spread(snap);
    let cx = (aabb[1] + aabb[3]) * 0.5;
    let cy = (aabb[0] + aabb[2]) * 0.5;
    pages
        .iter()
        .find(|p| {
            cx >= p.spread_origin.0
                && cx <= p.spread_origin.0 + p.width_pt
                && cy >= p.spread_origin.1
                && cy <= p.spread_origin.1 + p.height_pt
        })
        .cloned()
}

#[derive(Debug, Clone, Copy)]
struct AabbPt {
    top: f32,
    left: f32,
    bottom: f32,
    right: f32,
}

/// Phase H — single-member AABB in page-local pt for a known host
/// page. Per-member companion to `union_aabb_in_page`.
fn snapshot_aabb_in_page(
    snap: &crate::gesture::NodeSnapshot,
    host_page: &PageInfo,
) -> Option<AabbPt> {
    let [t, l, b, r] = snapshot_aabb_in_spread(snap);
    let left = l - host_page.spread_origin.0;
    let right = r - host_page.spread_origin.0;
    let top = t - host_page.spread_origin.1;
    let bottom = b - host_page.spread_origin.1;
    Some(AabbPt {
        top,
        left,
        bottom,
        right,
    })
}

#[allow(dead_code)]
fn union_aabb_in_page(
    snapshots: &[crate::gesture::NodeSnapshot],
    host_page: &PageInfo,
) -> Option<AabbPt> {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for snap in snapshots {
        let [t, l, b, r] = snapshot_aabb_in_spread(snap);
        let l = l - host_page.spread_origin.0;
        let r = r - host_page.spread_origin.0;
        let t = t - host_page.spread_origin.1;
        let b = b - host_page.spread_origin.1;
        min_x = min_x.min(l);
        max_x = max_x.max(r);
        min_y = min_y.min(t);
        max_y = max_y.max(b);
    }
    if !min_x.is_finite() {
        return None;
    }
    Some(AabbPt {
        top: min_y,
        left: min_x,
        bottom: max_y,
        right: max_x,
    })
}

fn snapshot_aabb_in_spread(snap: &crate::gesture::NodeSnapshot) -> [f32; 4] {
    let b = snap.bounds;
    let m = snap
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let corners = [
        apply(m, b.left, b.top),
        apply(m, b.right, b.top),
        apply(m, b.right, b.bottom),
        apply(m, b.left, b.bottom),
    ];
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in corners {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    [min_y, min_x, max_y, max_x]
}

fn apply(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn snap_targets_x(
    host_page: &PageInfo,
    siblings: &[FrameRect],
    moving: &[crate::gesture::NodeSnapshot],
) -> Vec<f32> {
    let moving_ids: std::collections::HashSet<&crate::element_selection::ElementId> =
        moving.iter().map(|s| &s.id).collect();
    let mut out = vec![0.0, host_page.width_pt * 0.5, host_page.width_pt];
    for f in siblings {
        if f.page_id != host_page.page_id {
            continue;
        }
        if moving_ids.contains(&f.element_id) {
            continue;
        }
        let [_top, left, _bottom, right] = f.aabb;
        out.push(left);
        out.push(right);
        out.push((left + right) * 0.5);
    }
    // Plan-2 §8.3 — vertical ruler guides snap on the x axis.
    out.extend_from_slice(&host_page.vertical_guides);
    out
}

fn snap_targets_y(
    host_page: &PageInfo,
    siblings: &[FrameRect],
    moving: &[crate::gesture::NodeSnapshot],
) -> Vec<f32> {
    let moving_ids: std::collections::HashSet<&crate::element_selection::ElementId> =
        moving.iter().map(|s| &s.id).collect();
    let mut out = vec![0.0, host_page.height_pt * 0.5, host_page.height_pt];
    for f in siblings {
        if f.page_id != host_page.page_id {
            continue;
        }
        if moving_ids.contains(&f.element_id) {
            continue;
        }
        let [top, _left, bottom, _right] = f.aabb;
        out.push(top);
        out.push(bottom);
        out.push((top + bottom) * 0.5);
    }
    // Plan-2 §8.3 — horizontal ruler guides snap on the y axis.
    out.extend_from_slice(&host_page.horizontal_guides);
    out
}

/// Phase H — like `snap_axis` but returns `(adjustment, target_pos)`
/// as a typed pair instead of an unzipped tuple. Used by the
/// multi-member snap pass to track the page each chosen target
/// belongs to.
fn snap_axis_match(candidates: &[f32], targets: &[f32], tolerance: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for &cand in candidates {
        for &target in targets {
            let diff = target - cand;
            if diff.abs() > tolerance {
                continue;
            }
            match best {
                None => best = Some((diff, target)),
                Some((cur, _)) if diff.abs() < cur.abs() => best = Some((diff, target)),
                _ => {}
            }
        }
    }
    best
}

/// Legacy single-axis snap (Phase E v1). Kept for the existing unit
/// tests that exercise it directly; the multi-member pass uses
/// `snap_axis_match` instead.
#[allow(dead_code)]
fn snap_axis(candidates: &[f32], targets: &[f32], tolerance: f32) -> (f32, Option<f32>) {
    let mut best: Option<(f32, f32)> = None; // (adjustment, target_pos)
    for &cand in candidates {
        for &target in targets {
            let diff = target - cand;
            if diff.abs() > tolerance {
                continue;
            }
            match best {
                None => best = Some((diff, target)),
                Some((cur, _)) if diff.abs() < cur.abs() => {
                    best = Some((diff, target));
                }
                _ => {}
            }
        }
    }
    match best {
        Some((adj, line)) => (adj, Some(line)),
        None => (0.0, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element_selection::ElementId;
    use crate::gesture::{GestureHandle, GestureModifiers, GestureType, NodeSnapshot};
    use paged_mutate::NodeId;
    use paged_parse::Bounds;

    fn snap_for(bounds: Bounds, kind: &str, id: &str) -> NodeSnapshot {
        NodeSnapshot {
            id: match kind {
                "tf" => ElementId::TextFrame(id.to_string()),
                "rect" => ElementId::Rectangle(id.to_string()),
                _ => panic!("unknown kind"),
            },
            node_id: match kind {
                "tf" => NodeId::TextFrame(id.to_string()),
                "rect" => NodeId::Rectangle(id.to_string()),
                _ => panic!("unknown kind"),
            },
            bounds,
            item_transform: None,
            image_item_transform: None,
            path_anchors: Vec::new(),
        }
    }

    fn session(snapshots: Vec<NodeSnapshot>) -> GestureSession {
        GestureSession {
            handle: GestureHandle(1),
            gesture: GestureType::Translate,
            snapshots,
            current_delta: None,
            modifiers: GestureModifiers::default(),
            anchor_spread: None,
            pivot_spread: None,
            camera_scale: None,
        }
    }

    fn session_at_scale(snapshots: Vec<NodeSnapshot>, scale: f32) -> GestureSession {
        let mut s = session(snapshots);
        s.camera_scale = Some(scale);
        s
    }

    fn page(id: &str, w: f32, h: f32) -> PageInfo {
        PageInfo {
            page_id: PageId(id.to_string()),
            width_pt: w,
            height_pt: h,
            spread_origin: (0.0, 0.0),
            vertical_guides: Vec::new(),
            horizontal_guides: Vec::new(),
        }
    }

    #[test]
    fn snap_to_page_left_edge() {
        // Frame at left=20, drag by dx=-19 → would land at left=1.
        // Tolerance 4 → snaps left edge to 0. Effective delta = -20.
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 20.0,
                bottom: 200.0,
                right: 200.0,
            },
            "tf",
            "u1",
        );
        let sess = session(vec![s]);
        let pages = vec![page("p1", 612.0, 792.0)];
        let adj = compute_snap_adjustment(&sess, (-19.0, 0.0), &pages, &[]);
        assert!((adj.delta.0 - -20.0).abs() < 1e-3, "{:?}", adj);
        assert_eq!(adj.delta.1, 0.0);
        assert_eq!(adj.lines.len(), 1);
        assert!(matches!(adj.lines[0].axis, SnapAxis::X));
        assert!((adj.lines[0].position - 0.0).abs() < 1e-3);
    }

    #[test]
    fn disable_snap_modifier_bypasses_snap_pass() {
        // Same setup as `snap_to_page_left_edge`: drag by dx=-19 with
        // tolerance 4 would normally snap left edge to 0 (effective
        // delta -20). With `disable_snap` set the raw delta passes
        // through and no snap lines surface.
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 20.0,
                bottom: 200.0,
                right: 200.0,
            },
            "tf",
            "u1",
        );
        let mut sess = session(vec![s]);
        sess.modifiers.disable_snap = true;
        let pages = vec![page("p1", 612.0, 792.0)];
        let adj = compute_snap_adjustment(&sess, (-19.0, 0.0), &pages, &[]);
        assert!((adj.delta.0 - -19.0).abs() < 1e-6);
        assert_eq!(adj.delta.1, 0.0);
        assert!(adj.lines.is_empty());
    }

    #[test]
    fn no_snap_when_out_of_tolerance() {
        // Move by dx = -10. Closest target (page left) at x=0 from
        // candidate 10 → diff 10 → outside 4pt tolerance.
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 20.0,
                bottom: 200.0,
                right: 200.0,
            },
            "tf",
            "u1",
        );
        let sess = session(vec![s]);
        let pages = vec![page("p1", 612.0, 792.0)];
        let adj = compute_snap_adjustment(&sess, (-10.0, 0.0), &pages, &[]);
        assert!((adj.delta.0 - -10.0).abs() < 1e-3);
        assert!(adj.lines.is_empty());
    }

    #[test]
    fn snap_to_sibling_frame_right_edge() {
        // Moving frame's left edge approaches sibling's right edge.
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 100.0,
                bottom: 200.0,
                right: 200.0,
            },
            "tf",
            "u1",
        );
        let sess = session(vec![s]);
        let pages = vec![page("p1", 612.0, 792.0)];
        let sibling = FrameRect {
            element_id: ElementId::Rectangle("u_sib".to_string()),
            page_id: PageId("p1".to_string()),
            aabb: [50.0, 30.0, 180.0, 75.0], // top, left, bottom, right
        };
        // moving.left = 100 + dx → want = 75 (sibling right) → dx = -25.
        // tolerance 4 → snap when dx between -29 and -21.
        let adj = compute_snap_adjustment(&sess, (-24.0, 0.0), &pages, &[sibling]);
        assert!((adj.delta.0 - -25.0).abs() < 1e-3, "{:?}", adj);
        assert!(adj.lines.iter().any(|l| matches!(l.axis, SnapAxis::X)));
    }

    #[test]
    fn smart_guides_surface_secondary_alignment() {
        // Sibling A: short rectangle at top (height 30). Sibling B:
        // tall rectangle whose right edge sits at exactly the same x
        // as A's right edge (smart-guide alignment in y wouldn't
        // help — A and B aren't on the same y-line; the guide we
        // want is the SECONDARY x-alignment surfaced after the
        // winner.
        //
        // Moving frame: small rect aligned with sibling A on x.
        // Drag dy=-19 toward the page-top edge → snap wins on Y
        // pulling top to 0. The X axis isn't snapped (dx=0), but
        // the moving frame already lines up with A's left edge AND
        // B's right edge → smart guides should surface both.
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 50.0,
                bottom: 130.0,
                right: 100.0,
            },
            "tf",
            "u_move",
        );
        let sess = session(vec![s]);
        let pages = vec![page("p1", 612.0, 792.0)];
        let sib_a = FrameRect {
            element_id: ElementId::Rectangle("u_a".to_string()),
            page_id: PageId("p1".to_string()),
            aabb: [10.0, 50.0, 40.0, 200.0], // top, left, bottom, right
        };
        let sib_b = FrameRect {
            element_id: ElementId::Rectangle("u_b".to_string()),
            page_id: PageId("p1".to_string()),
            aabb: [400.0, 0.0, 500.0, 100.0], // right edge at x=100
        };
        // dy=-99 → moving.top = 100-99 = 1, within 4 pt of page top
        // (0) → snap pulls top to 0 (adjusted dy = -100).
        let adj = compute_snap_adjustment(&sess, (0.0, -99.0), &pages, &[sib_a, sib_b]);
        // Y snap winner exists.
        let y_lines: Vec<&SnapLine> = adj
            .lines
            .iter()
            .filter(|l| matches!(l.axis, SnapAxis::Y))
            .collect();
        assert!(!y_lines.is_empty(), "expected at least one Y snap line");
        // First Y line is the winner (top → 0).
        assert!((y_lines[0].position - 0.0).abs() < 1e-3);
        // Smart guides: dx=0 doesn't snap on X, but the moving
        // frame's left=50 lines up with sib_a's left=50, AND
        // moving's right=100 lines up with sib_b's right=100.
        // Both should surface as X snap lines.
        let x_positions: Vec<f32> = adj
            .lines
            .iter()
            .filter(|l| matches!(l.axis, SnapAxis::X))
            .map(|l| l.position)
            .collect();
        assert!(
            x_positions.iter().any(|&p| (p - 50.0).abs() < 1e-3),
            "expected smart-guide at x=50 (left-edge alignment), got {x_positions:?}",
        );
        assert!(
            x_positions.iter().any(|&p| (p - 100.0).abs() < 1e-3),
            "expected smart-guide at x=100 (right-edge alignment), got {x_positions:?}",
        );
    }

    #[test]
    fn vertical_ruler_guide_acts_as_snap_target() {
        // Frame at left=20; vertical guide at x=18. Drag dx=-1 →
        // candidate left lands at 19, within 4pt of the guide
        // (delta 1 < 4) → snap pulls left to 18 (effective dx=-2).
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 20.0,
                bottom: 200.0,
                right: 200.0,
            },
            "tf",
            "u1",
        );
        let sess = session(vec![s]);
        let mut pg = page("p1", 612.0, 792.0);
        pg.vertical_guides.push(18.0);
        let pages = vec![pg];
        let adj = compute_snap_adjustment(&sess, (-1.0, 0.0), &pages, &[]);
        assert!((adj.delta.0 - -2.0).abs() < 1e-3, "{:?}", adj);
        assert!(adj
            .lines
            .iter()
            .any(|l| matches!(l.axis, SnapAxis::X) && (l.position - 18.0).abs() < 1e-3));
    }

    #[test]
    fn moving_frame_excluded_from_its_own_snap_targets() {
        // The moving frame's own bounds must NOT appear as a target —
        // otherwise dx=0 would always snap to dx=0 and the user could
        // never move it.
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 100.0,
                bottom: 200.0,
                right: 200.0,
            },
            "tf",
            "u1",
        );
        let sess = session(vec![s.clone()]);
        let pages = vec![page("p1", 1000.0, 1000.0)];
        let same_as_moving = FrameRect {
            element_id: ElementId::TextFrame("u1".to_string()),
            page_id: PageId("p1".to_string()),
            aabb: [100.0, 100.0, 200.0, 200.0],
        };
        // Drag by 3 — well within tolerance to "snap back" if the
        // moving frame's own edges were targets.
        let adj = compute_snap_adjustment(&sess, (3.0, 0.0), &pages, &[same_as_moving]);
        assert!(
            (adj.delta.0 - 3.0).abs() < 1e-3,
            "moving frame should not snap to itself: {:?}",
            adj
        );
    }

    fn page_at(id: &str, origin: (f32, f32), w: f32, h: f32) -> PageInfo {
        PageInfo {
            page_id: PageId(id.to_string()),
            width_pt: w,
            height_pt: h,
            spread_origin: origin,
            vertical_guides: Vec::new(),
            horizontal_guides: Vec::new(),
        }
    }

    #[test]
    fn multi_page_selection_snaps_against_per_member_pages() {
        // Two members on two different pages. Member A on page1
        // (origin 0,0) wants to snap its right edge to page1's right
        // edge (612). Member B on page2 (origin 612, 0) has plenty
        // of room; should NOT drag any snap.
        // After per-page snap, only A's edge snaps and the resulting
        // snap line is tagged with page1, not page2.
        let a = NodeSnapshot {
            id: ElementId::TextFrame("a".into()),
            node_id: NodeId::TextFrame("a".into()),
            // Page1 spread coords (0..612). Right edge at 610, snap
            // to 612 needs +2pt of dx.
            bounds: Bounds {
                top: 100.0,
                left: 500.0,
                bottom: 200.0,
                right: 610.0,
            },
            item_transform: None,
            image_item_transform: None,
            path_anchors: Vec::new(),
        };
        let b = NodeSnapshot {
            id: ElementId::TextFrame("b".into()),
            node_id: NodeId::TextFrame("b".into()),
            // Page2 spread coords (612..1224). Comfortably middle.
            bounds: Bounds {
                top: 100.0,
                left: 800.0,
                bottom: 200.0,
                right: 900.0,
            },
            item_transform: None,
            image_item_transform: None,
            path_anchors: Vec::new(),
        };
        let sess = session(vec![a, b]);
        let pages = vec![
            page_at("p1", (0.0, 0.0), 612.0, 792.0),
            page_at("p2", (612.0, 0.0), 612.0, 792.0),
        ];
        // dx = +1 — A's right candidate lands at 611 (within 4pt of
        // page1 right edge 612 → snaps to +2 total).
        let adj = compute_snap_adjustment(&sess, (1.0, 0.0), &pages, &[]);
        assert!((adj.delta.0 - 2.0).abs() < 1e-3, "{:?}", adj);
        // The snap line is on page1, not page2 (where neither edge
        // is near a target).
        let x_line = adj.lines.iter().find(|l| matches!(l.axis, SnapAxis::X));
        let x_line = x_line.expect("expected x snap line");
        assert_eq!(x_line.page_id, PageId("p1".into()));
        assert!((x_line.position - 612.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_scales_the_doc_space_tolerance() {
        // At camera scale = 2 (zoomed in 2×), the doc-space tolerance
        // halves to 2pt. A 3pt-near-edge candidate that snaps at scale
        // 1 should NOT snap at scale 2.
        let s = snap_for(
            Bounds {
                top: 100.0,
                left: 20.0,
                bottom: 200.0,
                right: 200.0,
            },
            "tf",
            "u1",
        );
        let pages = vec![page("p1", 612.0, 792.0)];
        // dx = -17 → candidate left = 3, diff = -3 from page-left (0).
        // |diff| = 3pt is inside the 4pt tolerance at scale=1
        // (snaps) but outside the 2pt tolerance at scale=2 (doesn't).
        let snap_at_1 =
            compute_snap_adjustment(&session(vec![s.clone()]), (-17.0, 0.0), &pages, &[]);
        assert!(
            !snap_at_1.lines.is_empty(),
            "should snap at scale 1: {:?}",
            snap_at_1
        );

        let snap_at_2 =
            compute_snap_adjustment(&session_at_scale(vec![s], 2.0), (-17.0, 0.0), &pages, &[]);
        assert!(
            snap_at_2.lines.is_empty(),
            "should NOT snap at scale 2: {:?}",
            snap_at_2
        );
    }

    #[test]
    fn non_translate_gestures_pass_through() {
        let s = snap_for(
            Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 100.0,
                right: 100.0,
            },
            "tf",
            "u1",
        );
        let mut sess = session(vec![s]);
        sess.gesture = GestureType::Rotate;
        let pages = vec![page("p1", 612.0, 792.0)];
        let adj = compute_snap_adjustment(&sess, (3.0, 3.0), &pages, &[]);
        assert_eq!(adj.delta, (3.0, 3.0));
        assert!(adj.lines.is_empty());
    }
}
