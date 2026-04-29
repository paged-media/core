//! Smart guides + snapping.
//!
//! The Select tool consults `compute_snap` during a drag: given the
//! moving frame's current bbox, the function returns a snap delta
//! (small dx/dy that nudges the frame onto a guide) plus the guide
//! segments the editor should paint as feedback.
//!
//! M3 ships axis-aligned snapping against:
//!  * other-frame edges + centerlines on the same spread
//!  * page margins (top / left / bottom / right)
//!  * page centerlines
//!
//! Snap distance is the threshold in pt within which a candidate
//! becomes the active snap. The editor passes a viewport-aware
//! threshold (typically `8 / zoom`) so the snap radius is constant
//! in screen space.

use idml_scene::Document;

use crate::hittest::AabbPt;

/// Result of a snap query. `delta_*` is the small translation that
/// should be applied to the moving frame to land on the snap target;
/// zero means "no snap engaged on this axis." `guides` is the set of
/// guide segments the editor should render.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapResult {
    pub delta_x_pt: f32,
    pub delta_y_pt: f32,
    pub guides: Vec<GuideSegment>,
}

/// A line in spread coordinates. Horizontal guides have y_a == y_b;
/// vertical guides have x_a == x_b.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideSegment {
    pub x_a: f32,
    pub y_a: f32,
    pub x_b: f32,
    pub y_b: f32,
}

/// Compute snap targets for `moving` against every other frame in
/// `spread_idx` plus the page margins / centerlines. `excluded` is
/// the moving frame's `Self` id so it doesn't snap to itself.
pub fn compute_snap(
    document: &Document,
    spread_idx: usize,
    moving: AabbPt,
    excluded_self_id: Option<&str>,
    threshold_pt: f32,
) -> SnapResult {
    let Some(ps) = document.spreads.get(spread_idx) else {
        return SnapResult::default();
    };
    let s = &ps.spread;

    // Collect candidate horizontal/vertical guides.
    let mut h_lines: Vec<f32> = Vec::new(); // y values that horizontal edges live at
    let mut v_lines: Vec<f32> = Vec::new();

    // Page edges + center for each page in the spread.
    for page in &s.pages {
        let pb = crate::hittest::transformed_bbox(page.bounds, page.item_transform);
        h_lines.extend([pb.y, pb.y + pb.h * 0.5, pb.y + pb.h]);
        v_lines.extend([pb.x, pb.x + pb.w * 0.5, pb.x + pb.w]);
    }

    // Other-frame edges + centers.
    let mut push_aabb = |a: &AabbPt| {
        h_lines.extend([a.y, a.y + a.h * 0.5, a.y + a.h]);
        v_lines.extend([a.x, a.x + a.w * 0.5, a.x + a.w]);
    };
    for f in &s.text_frames {
        if f.self_id.as_deref() == excluded_self_id {
            continue;
        }
        push_aabb(&crate::hittest::transformed_bbox(
            f.bounds,
            f.item_transform,
        ));
    }
    for r in &s.rectangles {
        if r.self_id.as_deref() == excluded_self_id {
            continue;
        }
        push_aabb(&crate::hittest::transformed_bbox(
            r.bounds,
            r.item_transform,
        ));
    }
    for o in &s.ovals {
        if o.self_id.as_deref() == excluded_self_id {
            continue;
        }
        push_aabb(&crate::hittest::transformed_bbox(
            o.bounds,
            o.item_transform,
        ));
    }

    // Candidate "moving" lines we test for proximity.
    let m_left = moving.x;
    let m_right = moving.x + moving.w;
    let m_cx = moving.x + moving.w * 0.5;
    let m_top = moving.y;
    let m_bot = moving.y + moving.h;
    let m_cy = moving.y + moving.h * 0.5;

    let (delta_x, snap_x) = best_snap(&[m_left, m_right, m_cx], &v_lines, threshold_pt);
    let (delta_y, snap_y) = best_snap(&[m_top, m_bot, m_cy], &h_lines, threshold_pt);

    let mut guides = Vec::new();
    if let Some(x) = snap_x {
        guides.push(GuideSegment {
            x_a: x,
            y_a: m_top.min(0.0).min(m_top - 50.0),
            x_b: x,
            y_b: m_bot.max(m_bot + 50.0),
        });
    }
    if let Some(y) = snap_y {
        guides.push(GuideSegment {
            x_a: m_left - 50.0,
            y_a: y,
            x_b: m_right + 50.0,
            y_b: y,
        });
    }

    SnapResult {
        delta_x_pt: delta_x,
        delta_y_pt: delta_y,
        guides,
    }
}

fn best_snap(probes: &[f32], targets: &[f32], threshold: f32) -> (f32, Option<f32>) {
    let mut best_delta = 0.0;
    let mut best_abs = threshold;
    let mut best_target: Option<f32> = None;
    for &p in probes {
        for &t in targets {
            let d = t - p;
            if d.abs() < best_abs {
                best_abs = d.abs();
                best_delta = d;
                best_target = Some(t);
            }
        }
    }
    (best_delta, best_target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_within_threshold_pulls_to_target() {
        let (delta, target) = best_snap(&[10.0], &[12.0, 100.0], 5.0);
        assert!((delta - 2.0).abs() < 1e-3);
        assert_eq!(target, Some(12.0));
    }

    #[test]
    fn snap_beyond_threshold_returns_none() {
        let (delta, target) = best_snap(&[10.0], &[100.0], 5.0);
        assert_eq!(delta, 0.0);
        assert!(target.is_none());
    }
}
