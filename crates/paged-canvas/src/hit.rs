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

//! Hit testing.
//!
//! Given a document-space point inside a known page, return the
//! topmost *selectable* element under the pointer. "Topmost" means the
//! same paint order the renderer follows — layer order first
//! (`paged_scene::layer_z_index`), then document order within a layer.
//!
//! Containment is **oriented**: the point is inverse-transformed into
//! each candidate frame's content-box space and tested against its raw
//! `bounds`. A 45°-rotated frame's empty AABB corners are correctly
//! excluded (AC-E-12).
//!
//! Layer gating mirrors the renderer's `visible && printable` rule for
//! visibility, and additionally drops items on `locked` layers — the
//! selection layer is the first consumer of `locked` (per IDML spec,
//! the renderer ignores it).
//!
//! Group descent: when iterating, group members are enqueued at the
//! group's document position. Group members' `item_transform` already
//! composes the group's transform (per
//! `crates/paged-parse/src/spread.rs:141-144`), so no additional
//! composition is needed. The topmost hit returned is the **leaf**, and
//! the containing group ancestry is reported via `group_chain` so the
//! UI can support an "enter group" gesture.

use std::collections::HashMap;

use paged_parse::{Bounds, FrameRef, Group, Spread};
use paged_renderer::{BuiltDocument, BuiltPage, LineLayout, PageId};

use crate::channel::HitFilter;
use crate::element_selection::ElementId;
use crate::model::CanvasModel;

/// Result of a hit test. Element-aware shape — the typed `element`
/// field is the new canonical identifier (Phase A), while
/// `frame_id` is kept as a back-compat alias for callers that haven't
/// migrated. `group_chain` is the outer-most group id first; empty
/// when the leaf is not nested in any group. `item_transform` is the
/// composed affine on the hit element so the overlay can draw an
/// oriented selection chrome.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitTestResult {
    pub element: Option<ElementId>,
    pub frame_id: Option<String>,
    pub story_id: Option<String>,
    /// AABB of the transformed corners, in page-local coords. Useful
    /// for callers that want a quick rectangle (e.g. a hover badge);
    /// for the oriented selection chrome use `bounds` + `item_transform`.
    pub frame_bounds: Option<[f32; 4]>,
    /// The frame's raw `GeometricBounds` (content-box space), in IDML
    /// `[top, left, bottom, right]` order. Combine with
    /// `item_transform` to draw an oriented box.
    pub bounds: Option<[f32; 4]>,
    pub item_transform: Option<[f32; 6]>,
    pub group_chain: Vec<String>,
    pub offset_within_story: Option<u32>,
    /// W3.A1 — set when the doc-point landed inside a table cell of the
    /// hit frame's story. Carries `(tableId, row, col)` so the canvas
    /// can select / mutate the cell. `None` when the hit frame has no
    /// table, or the point fell in the frame but outside the table grid.
    pub table_context: Option<TableHitContext>,
}

/// W3.A1 — the table cell a hit landed in. Page-local cell geometry is
/// retained on the `BuiltPage` at table-emit time (`cell_rects`); the
/// hit-tester resolves the topmost cell whose rect contains the
/// page-local point.
#[derive(Debug, Clone, PartialEq)]
pub struct TableHitContext {
    /// `<Table Self="…">` id.
    pub table_id: String,
    /// Zero-based row (template row; header / footer replays resolve to
    /// their source row).
    pub row: u32,
    /// Zero-based column.
    pub col: u32,
}

impl CanvasModel {
    /// Marquee-select every selectable element whose oriented bounds
    /// intersect `rect_in_page` (page-local coords,
    /// `[top, left, bottom, right]`). Returns element ids in paint
    /// order, top-first — same ordering as `hit_test`. Layer-visibility
    /// and locked gating mirror `hit_test`. Group descent already
    /// applied; returned ids are leaves.
    ///
    /// Intersection uses the Separating Axis Theorem so a marquee
    /// that crosses only an edge of a rotated frame (no corners
    /// inside) is correctly counted. AC-E-11.
    pub fn marquee_hits(&self, page_id: &PageId, rect_in_page: [f32; 4]) -> Vec<ElementId> {
        let Some(built_page) = self.page(page_id) else {
            return Vec::new();
        };
        let (page_origin_x, page_origin_y) = built_page.spread_origin;
        // page-local [top, left, bottom, right] → spread coords by
        // adding the page origin to the x/y axes. W1.9 — when the spread
        // carries a rotation/scale (`spread_transform`), first map the
        // marquee's four corners back through its inverse (page-local →
        // spread-inner) and take their AABB. For a rotated spread this is
        // a conservative over-approximation of the rotated marquee rect
        // (the marquee may select a few frames just outside the true
        // rotated box); exact OBB-vs-OBB marquee under spread rotation is
        // a follow-up. IDENTITY makes this the plain origin shift.
        let [t, l, bm, r] = rect_in_page;
        let corners = [(l, t), (r, t), (r, bm), (l, bm)];
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for &(px, py) in &corners {
            let (sx, sy) = invert_spread_transform(built_page.spread_transform, (px, py));
            min_x = min_x.min(sx);
            min_y = min_y.min(sy);
            max_x = max_x.max(sx);
            max_y = max_y.max(sy);
        }
        let rect_spread = [
            min_y + page_origin_y, // top
            min_x + page_origin_x, // left
            max_y + page_origin_y, // bottom
            max_x + page_origin_x, // right
        ];

        let scene = self.scene();
        let designmap = &scene.container.designmap;
        let layer_renders = paged_scene::build_layer_render_map(designmap);
        let layer_lockeds = paged_scene::build_layer_locked_map(designmap);
        let layer_z = paged_scene::layer_z_index(designmap);

        let mut hits: Vec<(usize, usize, ElementId)> = Vec::new();
        for parsed in &scene.spreads {
            let on_this_spread = parsed
                .spread
                .pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(page_id.as_str()) || p.self_id.is_none());
            if !on_this_spread {
                continue;
            }
            let candidates = collect_candidates(&parsed.spread, &layer_z);
            for c in &candidates {
                if !paged_scene::lookup_layer_render_visible(
                    &layer_renders,
                    c.item_layer.as_deref(),
                ) {
                    continue;
                }
                if paged_scene::lookup_layer_locked(&layer_lockeds, c.item_layer.as_deref()) {
                    continue;
                }
                if obb_intersects_aabb(c.bounds, c.item_transform, rect_spread) {
                    hits.push((c.layer_z, c.doc_order, c.element_id.clone()));
                }
            }
        }
        // Top-first: lower layer_z first, then later doc_order first.
        hits.sort_by_key(|(z, doc, _)| (*z, std::cmp::Reverse(*doc)));
        hits.into_iter().map(|(_, _, id)| id).collect()
    }

    /// Hit-test a document-space point inside a page. Returns the
    /// topmost selectable element under the point, in paint order.
    ///
    /// `doc_point` is in page-inner coords — the same space the
    /// renderer's `BuiltPage::list` commands use.
    pub fn hit_test(&self, page_id: &PageId, doc_point: (f32, f32)) -> HitTestResult {
        self.hit_test_filtered(page_id, doc_point, HitFilter::Any)
    }

    /// Hit-test with an explicit filter. The text tool sends
    /// `HitFilter::Text`; the select tool sends `Frame` or `Any`.
    pub fn hit_test_filtered(
        &self,
        page_id: &PageId,
        doc_point: (f32, f32),
        filter: HitFilter,
    ) -> HitTestResult {
        let Some(built_page) = self.page(page_id) else {
            return HitTestResult::default();
        };
        let (page_origin_x, page_origin_y) = built_page.spread_origin;
        // W1.9 — undo the spread-level rotation/scale the renderer applied
        // about the page origin (`frame_outer_transform` =
        // spread_transform ∘ translate(-origin) ∘ item_transform), so the
        // page-local pointer lands back in spread-inner coords where the
        // candidate frames' `item_transform`s live. IDENTITY (the common
        // case) makes this the plain `doc_point + spread_origin` shift.
        let local = invert_spread_transform(built_page.spread_transform, doc_point);
        let spread_point = (local.0 + page_origin_x, local.1 + page_origin_y);

        let scene = self.scene();
        let designmap = &scene.container.designmap;
        let layer_renders = paged_scene::build_layer_render_map(designmap);
        let layer_lockeds = paged_scene::build_layer_locked_map(designmap);
        let layer_z = paged_scene::layer_z_index(designmap);

        for parsed in &scene.spreads {
            let on_this_spread = parsed
                .spread
                .pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(page_id.as_str()) || p.self_id.is_none());
            if !on_this_spread {
                continue;
            }

            let mut candidates = collect_candidates(&parsed.spread, &layer_z);
            // Top-first: lower layer_z (top layer) first, then later
            // doc_order (last-painted) first.
            candidates.sort_by_key(|c| (c.layer_z, std::cmp::Reverse(c.doc_order)));

            for c in &candidates {
                if !paged_scene::lookup_layer_render_visible(
                    &layer_renders,
                    c.item_layer.as_deref(),
                ) {
                    continue;
                }
                if paged_scene::lookup_layer_locked(&layer_lockeds, c.item_layer.as_deref()) {
                    continue;
                }
                if !filter_allows(filter, c) {
                    continue;
                }
                if !point_in_oriented_frame(spread_point, c.bounds, c.item_transform) {
                    continue;
                }

                let offset = c.parent_story.as_deref().and_then(|sid| {
                    story_offset_at_point(
                        self.built(),
                        sid,
                        page_id,
                        Some(c.element_id.raw_id()),
                        doc_point,
                    )
                });
                // W3.A1 — resolve the table cell under the pointer, if
                // the hit frame's story owns a table whose cells landed
                // on this page. `doc_point` and `cell_rects` are both
                // page-local pt.
                let table_context = c
                    .parent_story
                    .as_deref()
                    .and_then(|sid| table_cell_at_point(built_page, sid, doc_point));
                let bbox = transform_bbox(c.bounds, c.item_transform);
                return HitTestResult {
                    element: Some(c.element_id.clone()),
                    frame_id: Some(c.element_id.raw_id().to_string()),
                    story_id: c.parent_story.clone(),
                    frame_bounds: Some(bbox_to_page_local(bbox, page_origin_x, page_origin_y)),
                    bounds: Some([c.bounds.top, c.bounds.left, c.bounds.bottom, c.bounds.right]),
                    item_transform: c.item_transform,
                    group_chain: c.group_chain.clone(),
                    offset_within_story: offset,
                    table_context,
                };
            }
        }
        HitTestResult::default()
    }
}

/// W1.9 — map a page-local point back through the inverse of the page's
/// spread-level rotation/scale (`spread_transform`, applied by the
/// renderer about the page origin). For the common identity transform
/// this returns the point unchanged, so the hit-test math is exactly the
/// pre-W1.9 plain origin shift. A singular transform (never produced by a
/// real IDML spread) also falls through unchanged.
fn invert_spread_transform(
    spread_transform: paged_compose::Transform,
    point: (f32, f32),
) -> (f32, f32) {
    if spread_transform == paged_compose::Transform::IDENTITY {
        return point;
    }
    match spread_transform.inverse() {
        Some(inv) => inv.apply(point.0, point.1),
        None => point,
    }
}

fn filter_allows(filter: HitFilter, c: &Candidate) -> bool {
    match filter {
        HitFilter::Any | HitFilter::Frame => true,
        HitFilter::Text => c.is_text,
    }
}

/// W3.A1 — find the table cell whose retained page-local rect contains
/// `doc_point` (page-local pt), restricting to cells of `story_id`.
/// Returns the LAST matching cell in emit order — header / footer
/// replays are pushed after body rows, but a body row and a replayed
/// header never overlap geometrically, so the order doesn't change the
/// result; the explicit last-match is just a deterministic tiebreak for
/// the (degenerate) zero-area-rect case. `None` when the point isn't in
/// any of this story's cells.
fn table_cell_at_point(
    page: &BuiltPage,
    story_id: &str,
    doc_point: (f32, f32),
) -> Option<TableHitContext> {
    let (px, py) = doc_point;
    let mut found: Option<TableHitContext> = None;
    for cr in &page.cell_rects {
        if cr.story_id != story_id {
            continue;
        }
        let [x, y, w, h] = cr.rect;
        if px >= x && px < x + w && py >= y && py < y + h {
            found = Some(TableHitContext {
                table_id: cr.table_id.clone(),
                row: cr.row,
                col: cr.col,
            });
        }
    }
    found
}

/// One selectable item, in spread-coord geometry.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub element_id: ElementId,
    pub bounds: Bounds,
    pub item_transform: Option<[f32; 6]>,
    pub item_layer: Option<String>,
    pub is_text: bool,
    pub parent_story: Option<String>,
    pub group_chain: Vec<String>,
    pub layer_z: usize,
    pub doc_order: usize,
}

pub(crate) fn collect_candidates(
    spread: &Spread,
    layer_z: &HashMap<&str, usize>,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut doc_order: usize = 0;

    // The parser fills `frames_in_order` with top-level frames + group
    // markers in XML order (group members live inside the group, not
    // in this vec). Legacy fixtures whose parser revision predates the
    // field fall back to a flat per-kind concatenation.
    let order: Vec<FrameRef> = if !spread.frames_in_order.is_empty() {
        spread.frames_in_order.clone()
    } else {
        let mut v: Vec<FrameRef> = Vec::new();
        for i in 0..spread.text_frames.len() {
            v.push(FrameRef::TextFrame(i));
        }
        for i in 0..spread.rectangles.len() {
            v.push(FrameRef::Rectangle(i));
        }
        for i in 0..spread.ovals.len() {
            v.push(FrameRef::Oval(i));
        }
        for i in 0..spread.graphic_lines.len() {
            v.push(FrameRef::GraphicLine(i));
        }
        for i in 0..spread.polygons.len() {
            v.push(FrameRef::Polygon(i));
        }
        v
    };

    for fr in &order {
        push_frame_ref(spread, *fr, &[], layer_z, &mut out, &mut doc_order);
    }
    out
}

fn push_frame_ref(
    spread: &Spread,
    fr: FrameRef,
    group_chain: &[String],
    layer_z: &HashMap<&str, usize>,
    out: &mut Vec<Candidate>,
    doc_order: &mut usize,
) {
    match fr {
        FrameRef::TextFrame(i) => {
            if let Some(f) = spread.text_frames.get(i) {
                let Some(id) = f.self_id.as_ref() else {
                    return;
                };
                let z = lookup_z(layer_z, f.item_layer.as_deref());
                out.push(Candidate {
                    element_id: ElementId::TextFrame(id.clone()),
                    bounds: f.bounds,
                    item_transform: f.item_transform,
                    item_layer: f.item_layer.clone(),
                    is_text: f.parent_story.is_some(),
                    parent_story: f.parent_story.clone(),
                    group_chain: group_chain.to_vec(),
                    layer_z: z,
                    doc_order: *doc_order,
                });
                *doc_order += 1;
            }
        }
        FrameRef::Rectangle(i) => {
            if let Some(f) = spread.rectangles.get(i) {
                let Some(id) = f.self_id.as_ref() else {
                    return;
                };
                let z = lookup_z(layer_z, f.item_layer.as_deref());
                out.push(Candidate {
                    element_id: ElementId::Rectangle(id.clone()),
                    bounds: f.bounds,
                    item_transform: f.item_transform,
                    item_layer: f.item_layer.clone(),
                    is_text: false,
                    parent_story: None,
                    group_chain: group_chain.to_vec(),
                    layer_z: z,
                    doc_order: *doc_order,
                });
                *doc_order += 1;
            }
        }
        FrameRef::Oval(i) => {
            if let Some(f) = spread.ovals.get(i) {
                let Some(id) = f.self_id.as_ref() else {
                    return;
                };
                let z = lookup_z(layer_z, f.item_layer.as_deref());
                out.push(Candidate {
                    element_id: ElementId::Oval(id.clone()),
                    bounds: f.bounds,
                    item_transform: f.item_transform,
                    item_layer: f.item_layer.clone(),
                    is_text: false,
                    parent_story: None,
                    group_chain: group_chain.to_vec(),
                    layer_z: z,
                    doc_order: *doc_order,
                });
                *doc_order += 1;
            }
        }
        FrameRef::GraphicLine(i) => {
            if let Some(f) = spread.graphic_lines.get(i) {
                let Some(id) = f.self_id.as_ref() else {
                    return;
                };
                let z = lookup_z(layer_z, f.item_layer.as_deref());
                out.push(Candidate {
                    element_id: ElementId::GraphicLine(id.clone()),
                    bounds: f.bounds,
                    item_transform: f.item_transform,
                    item_layer: f.item_layer.clone(),
                    is_text: false,
                    parent_story: None,
                    group_chain: group_chain.to_vec(),
                    layer_z: z,
                    doc_order: *doc_order,
                });
                *doc_order += 1;
            }
        }
        FrameRef::Polygon(i) => {
            if let Some(f) = spread.polygons.get(i) {
                let Some(id) = f.self_id.as_ref() else {
                    return;
                };
                let z = lookup_z(layer_z, f.item_layer.as_deref());
                out.push(Candidate {
                    element_id: ElementId::Polygon(id.clone()),
                    bounds: f.bounds,
                    item_transform: f.item_transform,
                    item_layer: f.item_layer.clone(),
                    is_text: false,
                    parent_story: None,
                    group_chain: group_chain.to_vec(),
                    layer_z: z,
                    doc_order: *doc_order,
                });
                *doc_order += 1;
            }
        }
        FrameRef::Group(i) => {
            if let Some(g) = spread.groups.get(i) {
                push_group_members(spread, g, group_chain, layer_z, out, doc_order);
            }
        }
    }
}

fn push_group_members(
    spread: &Spread,
    group: &Group,
    parent_chain: &[String],
    layer_z: &HashMap<&str, usize>,
    out: &mut Vec<Candidate>,
    doc_order: &mut usize,
) {
    let mut chain: Vec<String> = parent_chain.to_vec();
    if let Some(id) = group.self_id.as_ref() {
        chain.push(id.clone());
    }
    for member in &group.members {
        push_frame_ref(spread, *member, &chain, layer_z, out, doc_order);
    }
}

fn lookup_z(layer_z: &HashMap<&str, usize>, item_layer_ref: Option<&str>) -> usize {
    match item_layer_ref {
        Some(id) => layer_z.get(id).copied().unwrap_or(usize::MAX),
        None => usize::MAX,
    }
}

/// Invert a 2D affine `[a b c d tx ty]`. Returns `None` for a
/// degenerate matrix (zero determinant — shouldn't occur in well-formed
/// IDML but we handle it defensively).
pub(crate) fn invert_affine(m: [f32; 6]) -> Option<[f32; 6]> {
    let [a, b, c, d, tx, ty] = m;
    let det = a * d - b * c;
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let inv_det = 1.0 / det;
    let ia = d * inv_det;
    let ib = -b * inv_det;
    let ic = -c * inv_det;
    let id = a * inv_det;
    let itx = -(ia * tx + ic * ty);
    let ity = -(ib * tx + id * ty);
    Some([ia, ib, ic, id, itx, ity])
}

/// True iff `point` (in spread/world coords) lies within the oriented
/// rectangle defined by `bounds` (content-box coords) and the
/// composed `item_transform`. Matches the un-rotated AABB result for
/// transform-free or pure-translation matrices.
pub(crate) fn point_in_oriented_frame(
    point: (f32, f32),
    bounds: Bounds,
    item_transform: Option<[f32; 6]>,
) -> bool {
    let local = match item_transform {
        Some(m) => match invert_affine(m) {
            Some(inv) => apply_matrix(&inv, point.0, point.1),
            // Degenerate matrix — fall back to AABB so we don't reject
            // a hit on a frame whose transform parser declared zero
            // scale by mistake.
            None => point,
        },
        None => point,
    };
    local.0 >= bounds.left
        && local.0 <= bounds.right
        && local.1 >= bounds.top
        && local.1 <= bounds.bottom
}

/// Apply a 2D affine to the four corners of `b` and return the
/// axis-aligned bbox of the transformed corners. Kept for callers that
/// only want a quick rectangle (e.g. the `frame_bounds` field of
/// `HitTestResult`).
pub(crate) fn transform_bbox(b: Bounds, m: Option<[f32; 6]>) -> Bounds {
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

pub(crate) fn apply_matrix(m: &[f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Step 5 — closest anchor / Bezier-handle hit for path-edit mode.
///
/// `point_in_page` is page-local pt (the caller already shifted by
/// the page origin). `tol_pt` is the hit radius in doc-space pt —
/// the caller typically passes `tol_px / camera_scale` so the
/// effective hit area stays a constant screen size at any zoom.
///
/// Walks every anchor's three points (the anchor itself and its two
/// Bezier handles), projects them through the polygon's
/// `item_transform`, and returns the closest within `tol_pt`. Ties
/// break by role priority (anchor > right > left) so dragging on
/// top of an anchor doesn't accidentally grab a handle.
pub fn hit_path_anchor(
    anchors: &[(f32, f32, f32, f32, f32, f32)],
    item_transform: Option<[f32; 6]>,
    point_in_page: (f32, f32),
    tol_pt: f32,
) -> Option<paged_mutate::PathPointAddress> {
    let tol_sq = tol_pt * tol_pt;
    let identity = [1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    let m = item_transform.unwrap_or(identity);
    let mut best: Option<(f32, paged_mutate::PathPointAddress, u8)> = None;
    for (i, &(ax, ay, lx, ly, rx, ry)) in anchors.iter().enumerate() {
        let cands = [
            // (role, world-x, world-y, role-priority — higher wins on tie)
            (
                paged_mutate::PathPointRole::Anchor,
                apply_matrix(&m, ax, ay),
                2_u8,
            ),
            (
                paged_mutate::PathPointRole::Right,
                apply_matrix(&m, rx, ry),
                1_u8,
            ),
            (
                paged_mutate::PathPointRole::Left,
                apply_matrix(&m, lx, ly),
                0_u8,
            ),
        ];
        for (role, (wx, wy), prio) in cands {
            let dx = wx - point_in_page.0;
            let dy = wy - point_in_page.1;
            let d2 = dx * dx + dy * dy;
            if d2 > tol_sq {
                continue;
            }
            let addr = paged_mutate::PathPointAddress { index: i, role };
            match best {
                Some((bd, _, bp)) if bd < d2 || (bd == d2 && bp >= prio) => {}
                _ => best = Some((d2, addr, prio)),
            }
        }
    }
    best.map(|(_, addr, _)| addr)
}

/// Separating Axis Theorem: do the OBB defined by (`bounds`,
/// `item_transform`) and the AABB `aabb = [top, left, bottom, right]`
/// (both in the same coordinate space) intersect? Catches edge-only
/// overlaps that a simpler any-corner-inside test would miss.
pub(crate) fn obb_intersects_aabb(
    bounds: Bounds,
    item_transform: Option<[f32; 6]>,
    aabb: [f32; 4],
) -> bool {
    let [top, left, bottom, right] = aabb;
    let m = item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let obb = [
        apply_matrix(&m, bounds.left, bounds.top),
        apply_matrix(&m, bounds.right, bounds.top),
        apply_matrix(&m, bounds.right, bounds.bottom),
        apply_matrix(&m, bounds.left, bounds.bottom),
    ];
    let aabb_corners = [(left, top), (right, top), (right, bottom), (left, bottom)];
    // Test four candidate separating axes: the two AABB world axes
    // plus the two OBB edge directions (the transformed x and y unit
    // vectors). For axis-aligned + axis-aligned this collapses to a
    // standard rectangle overlap; for rotated OBBs the OBB axes catch
    // edge-only intersections.
    let axes = [
        (1.0_f32, 0.0_f32),
        (0.0_f32, 1.0_f32),
        (m[0], m[1]),
        (m[2], m[3]),
    ];
    for axis in axes {
        if axis.0 == 0.0 && axis.1 == 0.0 {
            continue;
        }
        let (mut amin, mut amax) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in obb {
            let d = p.0 * axis.0 + p.1 * axis.1;
            amin = amin.min(d);
            amax = amax.max(d);
        }
        let (mut bmin, mut bmax) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in aabb_corners {
            let d = p.0 * axis.0 + p.1 * axis.1;
            bmin = bmin.min(d);
            bmax = bmax.max(d);
        }
        if amax < bmin || bmax < amin {
            return false;
        }
    }
    true
}

/// Convert a spread-coord bbox into a page-local bbox by subtracting
/// the page's origin in spread coords. Returns `[left, top, right, bottom]`.
pub(crate) fn bbox_to_page_local(b: Bounds, page_origin_x: f32, page_origin_y: f32) -> [f32; 4] {
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
/// Snap rules:
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
            (Some(f), Some(lf)) => f == lf,
            _ => true,
        })
        .collect();
    if lines.is_empty() {
        return Some(0);
    }

    let mut best: &LineLayout = lines[0];
    let mut best_distance = vertical_distance_to(best, doc_point.1);
    for line in &lines[1..] {
        let d = vertical_distance_to(line, doc_point.1);
        if d < best_distance {
            best = line;
            best_distance = d;
        }
    }

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
            let mid = c.x_pt + c.advance_pt * 0.5;
            return if x < mid { c.byte } else { next.byte };
        }
    }
    let last = *line.clusters.last().unwrap();
    let last_right = last.x_pt + last.advance_pt;
    if x >= last_right {
        line.byte_range.end
    } else {
        let mid = last.x_pt + last.advance_pt * 0.5;
        if x < mid {
            last.byte
        } else {
            line.byte_range.end
        }
    }
}

pub(crate) fn paragraph_byte_offset(
    built: &BuiltDocument,
    story_id: &str,
    paragraph_idx: u32,
) -> u32 {
    if paragraph_idx == 0 {
        return 0;
    }
    let mut total: u32 = 0;
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
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_identity() {
        let id = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let inv = invert_affine(id).unwrap();
        for i in 0..6 {
            assert!((inv[i] - id[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn invert_translation_round_trips() {
        let m = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        let inv = invert_affine(m).unwrap();
        // applying m then inv should be identity
        let p = (5.0, 7.0);
        let q = apply_matrix(&m, p.0, p.1);
        let r = apply_matrix(&inv, q.0, q.1);
        assert!((r.0 - p.0).abs() < 1e-4);
        assert!((r.1 - p.1).abs() < 1e-4);
    }

    #[test]
    fn invert_45_degree_rotation_round_trips() {
        // 45° rotation matrix [cos, sin, -sin, cos, 0, 0].
        let c = std::f32::consts::FRAC_PI_4.cos();
        let s = std::f32::consts::FRAC_PI_4.sin();
        let m = [c, s, -s, c, 0.0, 0.0];
        let inv = invert_affine(m).unwrap();
        let p = (3.0, 4.0);
        let q = apply_matrix(&m, p.0, p.1);
        let r = apply_matrix(&inv, q.0, q.1);
        assert!((r.0 - p.0).abs() < 1e-4);
        assert!((r.1 - p.1).abs() < 1e-4);
    }

    #[test]
    fn invert_degenerate_returns_none() {
        // det = a*d - b*c = 0
        assert!(invert_affine([0.0, 0.0, 0.0, 0.0, 0.0, 0.0]).is_none());
        assert!(invert_affine([1.0, 0.0, 1.0, 0.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn invert_spread_transform_identity_is_noop() {
        // W1.9 — the common (identity spread) case returns the point
        // unchanged, so hit-test math stays the plain origin shift.
        let p = (12.0, -7.0);
        let got = invert_spread_transform(paged_compose::Transform::IDENTITY, p);
        assert_eq!(got, p);
    }

    #[test]
    fn invert_spread_transform_undoes_rotation() {
        // 90° CW rotation (renderer applies x'=-y, y'=x). The inverse
        // maps the rotated point back: a frame painted at (-125, 125)
        // came from inner (125, 125).
        let s = paged_compose::Transform([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        let got = invert_spread_transform(s, (-125.0, 125.0));
        assert!((got.0 - 125.0).abs() < 1e-3, "x={}", got.0);
        assert!((got.1 - 125.0).abs() < 1e-3, "y={}", got.1);
    }

    fn b(top: f32, left: f32, bottom: f32, right: f32) -> Bounds {
        Bounds {
            top,
            left,
            bottom,
            right,
        }
    }

    #[test]
    fn unrotated_oriented_matches_aabb() {
        let bounds = b(0.0, 0.0, 100.0, 100.0);
        // No transform: behaves like the original AABB test.
        assert!(point_in_oriented_frame((50.0, 50.0), bounds, None));
        assert!(!point_in_oriented_frame((-1.0, 50.0), bounds, None));
        assert!(!point_in_oriented_frame((50.0, 101.0), bounds, None));
    }

    #[test]
    fn rotated_45_excludes_aabb_corner() {
        // A 100×100 frame, centered at origin, rotated 45° about (0,0).
        // Raw bounds in content-box: [-50..50]×[-50..50]. The AABB of
        // the rotated corners extends to ±70.7; a click at (60, 60)
        // lies in that AABB but **outside** the rotated rect.
        let bounds = b(-50.0, -50.0, 50.0, 50.0);
        let c = std::f32::consts::FRAC_PI_4.cos();
        let s = std::f32::consts::FRAC_PI_4.sin();
        let m = [c, s, -s, c, 0.0, 0.0];
        // (0, 0) is the rotation center and clearly inside.
        assert!(point_in_oriented_frame((0.0, 0.0), bounds, Some(m)));
        // (60, 60) is in the AABB of the rotated rect but not in the
        // rotated rect itself. AC-E-12.
        assert!(!point_in_oriented_frame((60.0, 60.0), bounds, Some(m)));
        // (50, 0) is on the right edge of the rotated rect in world
        // coords — i.e. corresponds to a point on the diagonal of the
        // raw bounds. Should hit.
        let q = apply_matrix(&m, 40.0, 0.0);
        assert!(point_in_oriented_frame(q, bounds, Some(m)));
    }

    #[test]
    fn lookup_z_defaults_for_unknown() {
        let map: HashMap<&str, usize> = HashMap::new();
        assert_eq!(lookup_z(&map, None), usize::MAX);
        assert_eq!(lookup_z(&map, Some("missing")), usize::MAX);
    }

    #[test]
    fn sat_aabb_overlaps_aabb() {
        let bounds = b(0.0, 0.0, 100.0, 100.0);
        // Identical rect → hit.
        assert!(obb_intersects_aabb(bounds, None, [0.0, 0.0, 100.0, 100.0]));
        // Partial overlap → hit.
        assert!(obb_intersects_aabb(
            bounds,
            None,
            [50.0, 50.0, 150.0, 150.0]
        ));
        // Disjoint to the right → miss.
        assert!(!obb_intersects_aabb(
            bounds,
            None,
            [0.0, 200.0, 100.0, 300.0]
        ));
        // Disjoint above → miss.
        assert!(!obb_intersects_aabb(
            bounds,
            None,
            [-200.0, 0.0, -100.0, 100.0]
        ));
    }

    #[test]
    fn sat_rotated_edge_only_intersection() {
        // 100×100 frame centered at origin, rotated 45°.
        let bounds = b(-50.0, -50.0, 50.0, 50.0);
        let c = std::f32::consts::FRAC_PI_4.cos();
        let s = std::f32::consts::FRAC_PI_4.sin();
        let m = [c, s, -s, c, 0.0, 0.0];
        // Marquee rect that straddles only an edge of the rotated rect.
        // The OBB extends to roughly y = ±70.7 along the world y-axis;
        // a rect from y=40 to y=90, x=-5 to x=5 sits in the AABB but
        // also touches the rotated edge.
        assert!(obb_intersects_aabb(
            bounds,
            Some(m),
            [40.0, -5.0, 90.0, 5.0]
        ));
        // A rect far outside even the AABB → miss.
        assert!(!obb_intersects_aabb(
            bounds,
            Some(m),
            [200.0, 200.0, 300.0, 300.0]
        ));
        // A rect that lies in the AABB-corner that the OBB does NOT
        // occupy (e.g. (60, 60)–(70, 70) — outside the rotated rect).
        // AABB test would say "maybe" but SAT correctly rules it out.
        assert!(!obb_intersects_aabb(
            bounds,
            Some(m),
            [60.0, 60.0, 70.0, 70.0]
        ));
    }

    #[test]
    fn sat_marquee_fully_inside_rotated() {
        let bounds = b(-50.0, -50.0, 50.0, 50.0);
        let c = std::f32::consts::FRAC_PI_4.cos();
        let s = std::f32::consts::FRAC_PI_4.sin();
        let m = [c, s, -s, c, 0.0, 0.0];
        // Tiny marquee centered at origin — fully inside the OBB.
        assert!(obb_intersects_aabb(bounds, Some(m), [-5.0, -5.0, 5.0, 5.0]));
    }

    #[test]
    fn path_anchor_picks_closest_role() {
        // One anchor at (0,0) with handles at (-10,0) and (10,0).
        let anchors = [(0.0_f32, 0.0, -10.0, 0.0, 10.0, 0.0)];
        // Click right on the anchor.
        let hit = super::hit_path_anchor(&anchors, None, (0.0, 0.0), 4.0).expect("anchor hit");
        assert_eq!(hit.index, 0);
        assert_eq!(hit.role, paged_mutate::PathPointRole::Anchor);
        // Click near the left handle.
        let hit = super::hit_path_anchor(&anchors, None, (-10.5, 0.0), 4.0).expect("left hit");
        assert_eq!(hit.role, paged_mutate::PathPointRole::Left);
        // Click far away — no hit.
        assert!(super::hit_path_anchor(&anchors, None, (50.0, 50.0), 4.0).is_none());
    }

    #[test]
    fn path_anchor_respects_item_transform() {
        // Anchor at (10, 0) in inner coords; transform shifts +5 in x.
        let anchors = [(10.0_f32, 0.0, 0.0, 0.0, 20.0, 0.0)];
        let m = [1.0, 0.0, 0.0, 1.0, 5.0, 0.0]; // translate +5x
                                                // World position of the anchor is (15, 0); the click must
                                                // land there, not at (10, 0).
        assert!(super::hit_path_anchor(&anchors, Some(m), (10.0, 0.0), 1.0).is_none());
        let hit = super::hit_path_anchor(&anchors, Some(m), (15.0, 0.0), 1.0)
            .expect("hit after transform");
        assert_eq!(hit.role, paged_mutate::PathPointRole::Anchor);
    }
}
