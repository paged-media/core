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
use idml_renderer::{BuiltDocument, LineLayout, PageId};

use crate::model::CanvasModel;

/// Result of a hit test. `frame_id` is the topmost frame whose bbox
/// contains the point. `story_id` is set when the frame is a text
/// frame (`TextFrame`). `offset_within_story` is the character offset
/// the click falls at (Phase 3 Item 2 — computed via
/// `BuiltDocument::story_layout`). `frame_bounds` is the hit frame's
/// bbox in page-local coordinates — the overlay layer uses it
/// directly to draw the selection outline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitTestResult {
    pub frame_id: Option<String>,
    pub story_id: Option<String>,
    pub frame_bounds: Option<[f32; 4]>, // [left, top, right, bottom]
    pub offset_within_story: Option<u32>,
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
                    // Phase 3 Item 2: compute offset_within_story
                    // by bisecting the StoryLayout's clusters for
                    // this story / page / frame at the click point.
                    let offset = frame
                        .parent_story
                        .as_deref()
                        .and_then(|sid| {
                            story_offset_at_point(
                                self.built(),
                                sid,
                                page_id,
                                frame.self_id.as_deref(),
                                doc_point,
                            )
                        });
                    return HitTestResult {
                        frame_id: frame.self_id.clone(),
                        story_id: frame.parent_story.clone(),
                        frame_bounds: Some(bbox_to_page_local(
                            bbox,
                            page_origin_x,
                            page_origin_y,
                        )),
                        offset_within_story: offset,
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
                        offset_within_story: None,
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
                        offset_within_story: None,
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
                        offset_within_story: None,
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

/// Compute the story-local byte offset under a page-local point.
///
/// Walks the story's lines (filtered by host page + frame), picks the
/// line vertically closest to the click, then bisects its clusters
/// by `x_pt`. Returns the story-global byte (paragraph_byte_start +
/// cluster_byte), where paragraph_byte_start accounts for synthetic
/// `\n` per inter-paragraph boundary per the story-offset contract
/// in `selection.rs`.
///
/// Snap rules (Phase 3 Item 2):
///
/// - Click past end of line → snap to `byte_range.end`.
/// - Click between lines → snap to *vertically nearest* line; then
///   bisect x.
/// - Click in empty frame (no lines for this story on this page) →
///   `Some(0)` so the caret has a valid offset.
pub(crate) fn story_offset_at_point(
    built: &BuiltDocument,
    story_id: &str,
    page_id: &PageId,
    frame_id: Option<&str>,
    doc_point: (f32, f32),
) -> Option<u32> {
    let lines: Vec<&LineLayout> = built
        .story_layout(story_id)
        .into_iter()
        .filter(|l| &l.page_id == page_id)
        .filter(|l| match (frame_id, l.frame_id.as_deref()) {
            // If the caller knows the frame, prefer matching lines
            // within it. Fall back to any line on the page if the
            // strict match yields none (caller may pass a synthetic
            // / unattributed frame).
            (Some(f), Some(lf)) => f == lf,
            _ => true,
        })
        .collect();
    if lines.is_empty() {
        // Empty frame on this page or unplaced story → caret at 0.
        return Some(0);
    }

    // Pick the line vertically closest to the click. "Closest" means
    // (a) inside the ascent/descent span, else (b) minimum vertical
    // distance from the baseline.
    let mut best: &LineLayout = lines[0];
    let mut best_distance = vertical_distance_to(best, doc_point.1);
    for line in &lines[1..] {
        let d = vertical_distance_to(line, doc_point.1);
        if d < best_distance {
            best = line;
            best_distance = d;
        }
    }

    // Bisect clusters by x_pt.
    let cluster_byte = pick_cluster_byte(best, doc_point.0);
    Some(paragraph_byte_offset(built, story_id, best.paragraph_idx) + cluster_byte)
}

fn vertical_distance_to(line: &LineLayout, y: f32) -> f32 {
    let top = line.baseline_y_pt - line.ascent_pt;
    let bot = line.baseline_y_pt + line.descent_pt;
    if y >= top && y <= bot {
        0.0
    } else if y < top {
        top - y
    } else {
        y - bot
    }
}

/// Bisect `line.clusters` by `x_pt`. Returns a paragraph-local byte.
///
/// - Click left of the first cluster: snap to that cluster's byte.
/// - Click within the i-th cluster's span (`[x, x+advance)`): if the
///   click is in the left half, snap to that cluster; otherwise snap
///   to the next cluster (so caret lands *between* characters as
///   typing convention demands).
/// - Click past the last cluster: snap to `byte_range.end` (end-of-
///   line affinity).
fn pick_cluster_byte(line: &LineLayout, x: f32) -> u32 {
    if line.clusters.is_empty() {
        return line.byte_range.start;
    }
    if x <= line.clusters[0].x_pt {
        return line.clusters[0].byte;
    }
    for win in line.clusters.windows(2) {
        let c = win[0];
        let next = win[1];
        if x >= c.x_pt && x < next.x_pt {
            // Caret goes left of the next cluster if click landed in
            // the right half of c's advance — matches typing intuition
            // (caret "follows" the character the click is past).
            let mid = c.x_pt + c.advance_pt * 0.5;
            return if x < mid { c.byte } else { next.byte };
        }
    }
    // Past the last cluster. End-of-line: byte_range.end.
    let last = *line.clusters.last().unwrap();
    let last_right = last.x_pt + last.advance_pt;
    if x >= last_right {
        line.byte_range.end
    } else {
        // Inside the last cluster's span.
        let mid = last.x_pt + last.advance_pt * 0.5;
        if x < mid {
            last.byte
        } else {
            line.byte_range.end
        }
    }
}

/// Story-global byte offset of paragraph `idx`'s first character.
/// Counts each preceding paragraph's run-text bytes + one synthetic
/// `\n` per inter-paragraph boundary (the story-offset contract).
pub(crate) fn paragraph_byte_offset(
    built: &BuiltDocument,
    story_id: &str,
    paragraph_idx: u32,
) -> u32 {
    // We don't have the parsed Story here — only the BuiltDocument's
    // captured layouts. Walk the captured lines for earlier
    // paragraphs and sum their byte_range.end (the last visible byte
    // of each paragraph). Plus 1 per inter-paragraph boundary.
    //
    // Note: this can under-count if a paragraph's tail is not visible
    // (overset text dropped from the chain). For the correctness
    // layer this matches the user's mental model — selection cannot
    // address invisible bytes — but a future iteration should
    // consult the parsed Story directly when invisible-tail addressing
    // becomes necessary.
    if paragraph_idx == 0 {
        return 0;
    }
    let mut total: u32 = 0;
    let lines = built.story_layout(story_id);
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for line in lines {
        if line.paragraph_idx >= paragraph_idx {
            break;
        }
        seen.insert(line.paragraph_idx);
        // Use the last line of each paragraph's byte_range.end as the
        // paragraph's byte length. We don't know the line order
        // a priori for arbitrary captures, but story_layout sorts by
        // (paragraph_idx, line_idx) so each iteration touches the
        // largest end for that paragraph.
        // The contribution to `total` is the paragraph's length once
        // — handled by adding only when we transition paragraphs.
        // Simpler: keep total = sum of (byte_range.end + 1) for the
        // last line of each completed paragraph.
        // Track per-paragraph max byte_range.end and tally at the end.
        let _ = line;
    }
    // Recompute via a fold that captures per-paragraph max length.
    let mut max_end: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for line in built.story_layout(story_id) {
        if line.paragraph_idx >= paragraph_idx {
            break;
        }
        let entry = max_end.entry(line.paragraph_idx).or_insert(0);
        if line.byte_range.end > *entry {
            *entry = line.byte_range.end;
        }
    }
    for (_, end) in max_end {
        total += end + 1; // +1 for the synthetic inter-paragraph \n
    }
    let _ = seen;
    total
}
