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

use idml_renderer::PageId;
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
    if let Some(m) = best_x {
        lines.push(SnapLine {
            axis: SnapAxis::X,
            position: m.target,
            page_id: m.page_id,
        });
    }
    if let Some(m) = best_y {
        lines.push(SnapLine {
            axis: SnapAxis::Y,
            position: m.target,
            page_id: m.page_id,
        });
    }
    SnapAdjustment {
        delta: (dx + adj_dx, dy + adj_dy),
        lines,
    }
}

#[derive(Debug, Clone)]
struct SnapMatch {
    adjustment: f32,
    target: f32,
    page_id: PageId,
}

/// Page summary fed into the snap pass.
#[derive(Debug, Clone)]
pub(crate) struct PageInfo {
    pub page_id: PageId,
    pub width_pt: f32,
    pub height_pt: f32,
    /// Page origin in spread coords (matches `BuiltPage::spread_origin`).
    pub spread_origin: (f32, f32),
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
            (a - 1.0).abs() < 1e-4
                && (d - 1.0).abs() < 1e-4
                && b.abs() < 1e-4
                && c.abs() < 1e-4
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
    let m = snap.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
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
    let mut out = vec![
        0.0,
        host_page.width_pt * 0.5,
        host_page.width_pt,
    ];
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
    out
}

fn snap_targets_y(
    host_page: &PageInfo,
    siblings: &[FrameRect],
    moving: &[crate::gesture::NodeSnapshot],
) -> Vec<f32> {
    let moving_ids: std::collections::HashSet<&crate::element_selection::ElementId> =
        moving.iter().map(|s| &s.id).collect();
    let mut out = vec![
        0.0,
        host_page.height_pt * 0.5,
        host_page.height_pt,
    ];
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
    out
}

/// Phase H — like `snap_axis` but returns `(adjustment, target_pos)`
/// as a typed pair instead of an unzipped tuple. Used by the
/// multi-member snap pass to track the page each chosen target
/// belongs to.
fn snap_axis_match(
    candidates: &[f32],
    targets: &[f32],
    tolerance: f32,
) -> Option<(f32, f32)> {
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
    use idml_mutate::NodeId;
    use idml_parse::Bounds;

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
        }
    }

    #[test]
    fn snap_to_page_left_edge() {
        // Frame at left=20, drag by dx=-19 → would land at left=1.
        // Tolerance 4 → snaps left edge to 0. Effective delta = -20.
        let s = snap_for(
            Bounds { top: 100.0, left: 20.0, bottom: 200.0, right: 200.0 },
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
            Bounds { top: 100.0, left: 20.0, bottom: 200.0, right: 200.0 },
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
            Bounds { top: 100.0, left: 20.0, bottom: 200.0, right: 200.0 },
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
            Bounds { top: 100.0, left: 100.0, bottom: 200.0, right: 200.0 },
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
    fn moving_frame_excluded_from_its_own_snap_targets() {
        // The moving frame's own bounds must NOT appear as a target —
        // otherwise dx=0 would always snap to dx=0 and the user could
        // never move it.
        let s = snap_for(
            Bounds { top: 100.0, left: 100.0, bottom: 200.0, right: 200.0 },
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
            bounds: Bounds { top: 100.0, left: 500.0, bottom: 200.0, right: 610.0 },
            item_transform: None,
            image_item_transform: None,
            path_anchors: Vec::new(),
        };
        let b = NodeSnapshot {
            id: ElementId::TextFrame("b".into()),
            node_id: NodeId::TextFrame("b".into()),
            // Page2 spread coords (612..1224). Comfortably middle.
            bounds: Bounds { top: 100.0, left: 800.0, bottom: 200.0, right: 900.0 },
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
            Bounds { top: 100.0, left: 20.0, bottom: 200.0, right: 200.0 },
            "tf",
            "u1",
        );
        let pages = vec![page("p1", 612.0, 792.0)];
        // dx = -17 → candidate left = 3, diff = -3 from page-left (0).
        // |diff| = 3pt is inside the 4pt tolerance at scale=1
        // (snaps) but outside the 2pt tolerance at scale=2 (doesn't).
        let snap_at_1 = compute_snap_adjustment(
            &session(vec![s.clone()]),
            (-17.0, 0.0),
            &pages,
            &[],
        );
        assert!(
            !snap_at_1.lines.is_empty(),
            "should snap at scale 1: {:?}",
            snap_at_1
        );

        let snap_at_2 = compute_snap_adjustment(
            &session_at_scale(vec![s], 2.0),
            (-17.0, 0.0),
            &pages,
            &[],
        );
        assert!(
            snap_at_2.lines.is_empty(),
            "should NOT snap at scale 2: {:?}",
            snap_at_2
        );
    }

    #[test]
    fn non_translate_gestures_pass_through() {
        let s = snap_for(
            Bounds { top: 0.0, left: 0.0, bottom: 100.0, right: 100.0 },
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
