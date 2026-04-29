//! Hit testing — point in spread/page coordinates → topmost frame.
//!
//! M1 ships a brute-force linear scan over frames in a single spread.
//! Pages typically have under a few hundred frames; the scan is sub-
//! millisecond and trivially correct. An R-tree is the right shape
//! once incremental display-list emit (M2+) lets us cache it across
//! frames; building one per hit-test on M1's small docs would only
//! add bookkeeping.
//!
//! Coordinates are in pt, in the *spread's* coordinate system (the
//! same system the renderer's display list uses pre-page-origin
//! subtraction). The caller is responsible for mapping a canvas-px
//! click through the viewport before calling `hit_test_spread`.

use idml_parse::Bounds;
use idml_scene::Document;

use crate::ids::NodeId;

/// A frame hit, returned in z-order priority.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameHit {
    pub frame: NodeId,
    /// Spread index (into `Document::spreads`).
    pub spread_idx: usize,
    /// Frame's axis-aligned bbox in spread coords. Useful to the
    /// editor for selection highlight even before the renderer hands
    /// out a fresh display list.
    pub bbox: AabbPt,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AabbPt {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl AabbPt {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Hit-test a single spread. Returns the topmost frame whose
/// transformed bbox contains `(x_pt, y_pt)`. "Topmost" follows IDML's
/// drawing order: later items in the spread's typed lists paint on
/// top, so iteration is reversed.
pub fn hit_test_spread(
    document: &Document,
    spread_idx: usize,
    x_pt: f32,
    y_pt: f32,
) -> Option<FrameHit> {
    let ps = document.spreads.get(spread_idx)?;
    let s = &ps.spread;

    // Collect every frame's transformed bbox + node id. Order matters
    // for top-most resolution; we follow IDML's standard draw order
    // (TextFrame, Rectangle, Oval, GraphicLine, Polygon — checked
    // against the renderer's pipeline order). Within each kind the
    // last item paints on top.
    //
    // Iteration is reversed within each kind, then across kinds, so
    // the first match is the topmost.
    let kinds = [
        FrameKindIter::Polygon(&s.polygons),
        FrameKindIter::GraphicLine(&s.graphic_lines),
        FrameKindIter::Oval(&s.ovals),
        FrameKindIter::Rectangle(&s.rectangles),
        FrameKindIter::Text(&s.text_frames),
    ];

    for kind in kinds {
        if let Some(hit) = kind.find_topmost(spread_idx, x_pt, y_pt) {
            return Some(hit);
        }
    }
    None
}

enum FrameKindIter<'a> {
    Text(&'a [idml_parse::TextFrame]),
    Rectangle(&'a [idml_parse::Rectangle]),
    Oval(&'a [idml_parse::Oval]),
    GraphicLine(&'a [idml_parse::GraphicLine]),
    Polygon(&'a [idml_parse::Polygon]),
}

impl<'a> FrameKindIter<'a> {
    fn find_topmost(&self, spread_idx: usize, x: f32, y: f32) -> Option<FrameHit> {
        match self {
            FrameKindIter::Text(v) => find_in_slice(spread_idx, x, y, v.iter().rev(), |f| {
                (f.self_id.as_deref(), f.bounds, f.item_transform)
            }),
            FrameKindIter::Rectangle(v) => find_in_slice(spread_idx, x, y, v.iter().rev(), |f| {
                (f.self_id.as_deref(), f.bounds, f.item_transform)
            }),
            FrameKindIter::Oval(v) => find_in_slice(spread_idx, x, y, v.iter().rev(), |f| {
                (f.self_id.as_deref(), f.bounds, f.item_transform)
            }),
            FrameKindIter::GraphicLine(v) => find_in_slice(spread_idx, x, y, v.iter().rev(), |f| {
                (f.self_id.as_deref(), f.bounds, f.item_transform)
            }),
            FrameKindIter::Polygon(v) => find_in_slice(spread_idx, x, y, v.iter().rev(), |f| {
                (f.self_id.as_deref(), f.bounds, f.item_transform)
            }),
        }
    }
}

fn find_in_slice<'a, T: 'a, I, F>(
    spread_idx: usize,
    x: f32,
    y: f32,
    iter: I,
    extract: F,
) -> Option<FrameHit>
where
    I: Iterator<Item = &'a T>,
    F: Fn(&'a T) -> (Option<&'a str>, Bounds, Option<[f32; 6]>),
{
    for item in iter {
        let (id_opt, bounds, xform) = extract(item);
        let bbox = transformed_bbox(bounds, xform);
        if bbox.contains(x, y) {
            let id = id_opt?.to_string();
            return Some(FrameHit {
                frame: NodeId::Frame(id),
                spread_idx,
                bbox,
            });
        }
    }
    None
}

/// Compute the axis-aligned bounding box of a frame's `bounds`
/// rectangle after applying its `ItemTransform`. The renderer
/// composes (page_origin × item_transform) for paint, but for hit-
/// testing in spread space we only need item_transform.
pub fn transformed_bbox(bounds: Bounds, xform: Option<[f32; 6]>) -> AabbPt {
    let (a, b, c, d, tx, ty) = match xform {
        Some([a, b, c, d, tx, ty]) => (a, b, c, d, tx, ty),
        None => (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
    };
    let pts = [
        (bounds.left, bounds.top),
        (bounds.right, bounds.top),
        (bounds.right, bounds.bottom),
        (bounds.left, bounds.bottom),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in pts {
        let sx = a * x + c * y + tx;
        let sy = b * x + d * y + ty;
        min_x = min_x.min(sx);
        min_y = min_y.min(sy);
        max_x = max_x.max(sx);
        max_y = max_y.max(sy);
    }
    AabbPt {
        x: min_x,
        y: min_y,
        w: max_x - min_x,
        h: max_y - min_y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_contains_works_at_edges() {
        let a = AabbPt {
            x: 10.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
        };
        assert!(a.contains(10.0, 20.0));
        assert!(a.contains(60.0, 50.0));
        assert!(!a.contains(9.99, 20.0));
        assert!(!a.contains(60.01, 35.0));
    }

    #[test]
    fn transformed_bbox_under_translation() {
        let b = Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 30.0,
            right: 50.0,
        };
        let t = transformed_bbox(b, Some([1.0, 0.0, 0.0, 1.0, 100.0, 200.0]));
        assert!((t.x - 100.0).abs() < 1e-3);
        assert!((t.y - 200.0).abs() < 1e-3);
        assert!((t.w - 50.0).abs() < 1e-3);
        assert!((t.h - 30.0).abs() < 1e-3);
    }

    #[test]
    fn transformed_bbox_under_90deg_rotation() {
        // 90°-CW rotation matrix: [0 1 -1 0 tx ty]
        let b = Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 30.0,
            right: 50.0,
        };
        let t = transformed_bbox(b, Some([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]));
        // After 90° CW the box becomes 30 wide × 50 tall (+ a y-shift
        // because the corners spread into negatives).
        assert!((t.w - 30.0).abs() < 1e-3);
        assert!((t.h - 50.0).abs() < 1e-3);
    }
}
