//! Hit testing.
//!
//! Phase E (Phase 1 polish) — given a document-space point inside a
//! known page, return the topmost frame whose bounding box contains
//! it. Linear scan over the page's spread for now; spec calls for an
//! R-tree once frame counts get into the thousands per spread, but at
//! today's working scale (≤200 frames per spread) the linear path is
//! single-digit microseconds and the R-tree adds memory + invalidation
//! cost.

use idml_parse::Bounds;
use idml_renderer::PageId;

use crate::model::CanvasModel;

/// Result of a hit test. `frame_id` is the topmost frame whose bbox
/// contains the point. `story_id` is set when the frame is a text
/// frame (`TextFrame`); future work extracts the offset-within-story
/// from glyph clusters. `frame_bounds` is the hit frame's bbox in
/// page-local coordinates — the overlay layer uses it directly to
/// draw the selection outline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitTestResult {
    pub frame_id: Option<String>,
    pub story_id: Option<String>,
    pub frame_bounds: Option<[f32; 4]>, // [left, top, right, bottom]
}

impl CanvasModel {
    /// Hit-test a document-space point inside a page. Returns the
    /// topmost frame (last-drawn wins; z-order is the order frames
    /// appear in the spread's `text_frames` vec for now).
    ///
    /// `doc_point` is in page-inner coords — the same coordinate
    /// space the renderer's `BuiltPage::list` commands use.
    pub fn hit_test(&self, page_id: &PageId, doc_point: (f32, f32)) -> HitTestResult {
        let Some(built_page) = self.page(page_id) else {
            return HitTestResult::default();
        };

        // Map the page-inner point into spread coords so we can
        // compare against frame bboxes whose `item_transform`s are
        // spread-relative.
        let (page_origin_x, page_origin_y) = built_page.spread_origin;
        let spread_point = (doc_point.0 + page_origin_x, doc_point.1 + page_origin_y);

        // Walk every spread looking for the one whose pages include
        // this page. The page's `id` is the IDML `Self` attribute;
        // we match against the spread's `pages` list.
        for parsed in &self.scene().spreads {
            let on_this_spread = parsed.spread.pages.iter().any(|p| {
                p.self_id.as_deref() == Some(page_id.as_str())
                    || p.self_id.is_none() // synthetic ids: fall through to bbox check
            });
            if !on_this_spread {
                continue;
            }
            // Walk text frames in *reverse* order so the topmost
            // (last-drawn) frame wins under overlapping bboxes —
            // matches the renderer's z-order convention.
            for frame in parsed.spread.text_frames.iter().rev() {
                let bbox = transform_bbox(frame.bounds, frame.item_transform);
                if point_in_bounds(spread_point, bbox) {
                    return HitTestResult {
                        frame_id: frame.self_id.clone(),
                        story_id: frame.parent_story.clone(),
                        frame_bounds: Some(bbox_to_page_local(
                            bbox,
                            page_origin_x,
                            page_origin_y,
                        )),
                    };
                }
            }
            // Then rectangles / ovals / polygons in z-order. Future
            // work may also include Group children once their
            // transform composition is plumbed through.
            for rect in parsed.spread.rectangles.iter().rev() {
                let bbox = transform_bbox(rect.bounds, rect.item_transform);
                if point_in_bounds(spread_point, bbox) {
                    return HitTestResult {
                        frame_id: rect.self_id.clone(),
                        story_id: None,
                        frame_bounds: Some(bbox_to_page_local(
                            bbox,
                            page_origin_x,
                            page_origin_y,
                        )),
                    };
                }
            }
            for oval in parsed.spread.ovals.iter().rev() {
                let bbox = transform_bbox(oval.bounds, oval.item_transform);
                if point_in_bounds(spread_point, bbox) {
                    return HitTestResult {
                        frame_id: oval.self_id.clone(),
                        story_id: None,
                        frame_bounds: Some(bbox_to_page_local(
                            bbox,
                            page_origin_x,
                            page_origin_y,
                        )),
                    };
                }
            }
            for poly in parsed.spread.polygons.iter().rev() {
                let bbox = transform_bbox(poly.bounds, poly.item_transform);
                if point_in_bounds(spread_point, bbox) {
                    return HitTestResult {
                        frame_id: poly.self_id.clone(),
                        story_id: None,
                        frame_bounds: Some(bbox_to_page_local(
                            bbox,
                            page_origin_x,
                            page_origin_y,
                        )),
                    };
                }
            }
        }
        HitTestResult::default()
    }
}

/// Apply a 2D affine to the four corners of `b` and return the
/// axis-aligned bbox of the transformed corners. Duplicates the
/// math from `idml_renderer::pipeline::transform_bounds`; pulling
/// it across would force `idml-renderer` to expose more internals
/// than the canvas needs.
fn transform_bbox(b: Bounds, m: Option<[f32; 6]>) -> Bounds {
    let Some(m) = m else {
        return b;
    };
    let corners = [
        apply_matrix(&m, b.left, b.top),
        apply_matrix(&m, b.right, b.top),
        apply_matrix(&m, b.right, b.bottom),
        apply_matrix(&m, b.left, b.bottom),
    ];
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for (x, y) in corners {
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }
    Bounds {
        top: min_y,
        left: min_x,
        bottom: max_y,
        right: max_x,
    }
}

fn apply_matrix(m: &[f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn point_in_bounds(p: (f32, f32), b: Bounds) -> bool {
    p.0 >= b.left && p.0 <= b.right && p.1 >= b.top && p.1 <= b.bottom
}

/// Convert a spread-coord bbox into a page-local bbox by subtracting
/// the page's origin in spread coords. Returns `[left, top, right, bottom]`.
fn bbox_to_page_local(b: Bounds, page_origin_x: f32, page_origin_y: f32) -> [f32; 4] {
    [
        b.left - page_origin_x,
        b.top - page_origin_y,
        b.right - page_origin_x,
        b.bottom - page_origin_y,
    ]
}
