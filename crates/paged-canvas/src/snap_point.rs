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

//! Point snapping (v67, RFI C-68) — the one resolver every surface uses.
//!
//! [`crate::snap`] snaps a translate gesture's EDGES against page and
//! sibling edges. Drawing and point editing need the other half: snap a
//! single POINT (a pen click, a dragged anchor, a resize corner) to the
//! points, lines and paths around it. Before v67 that half lived in
//! paged.draw and in the editor, which could only see the page and the
//! path being edited — every other object's geometry was a wire read
//! away. The engine holds the whole scene, so the targets live here:
//!
//! * **points** — every anchor of every visible leaf, a box-drawn
//!   frame's corners, an oval's four quadrant points, and each element's
//!   centre; the page's corners and centre;
//! * **alignment lines** — the x and y lines through each of those
//!   points (smart guides), the page's edges and centre lines, ruler
//!   guides, and the document grid;
//! * **segments** — the nearest point on any element's outline.
//!
//! Precedence is Illustrator's, as paged.draw's `snapPoint` had it: the
//! nearest POINT within tolerance wins outright; otherwise x and y snap
//! INDEPENDENTLY to the nearest alignment line; otherwise the nearest
//! point on a segment. Tolerance is in screen px, converted with the
//! caller's camera scale, so it is constant at every zoom.
//!
//! Targets are built once per document build ([`SnapIndex`], keyed by the
//! model's build generation) and reused by every query until the next
//! rebuild — a pointer move costs a scan of one page's targets, not a
//! walk of the document.

use std::collections::HashMap;

use paged_model::{Bounds, PathAnchor, Spread};
use paged_renderer::{BuiltDocument, PageId};
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::element_selection::ElementId;
use crate::snap::{SnapAxis, SnapLine};

/// The document's snapping preferences — one set for every tool and
/// gesture, so a move and a pen click agree on what "close" means.
/// Session state, not document state: it is not saved and not undoable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapSettings {
    /// Master switch. Off ⇒ no gesture snaps and every query answers the
    /// point it was given.
    pub enabled: bool,
    /// Tolerance in CSS px, converted to pt through the camera scale.
    pub tolerance_px: f32,
    /// Anchors, frame corners, oval quadrant points and element centres.
    pub points: bool,
    /// The x / y lines through those points (smart guides).
    pub alignment: bool,
    /// The nearest point on an element's outline.
    pub segments: bool,
    /// Page corners, centre, edges and centre lines.
    pub page: bool,
    /// Ruler guides.
    pub guides: bool,
    /// The document grid (`GridPreference` gridline divisions). Off by
    /// default, as in InDesign.
    pub grid: bool,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            tolerance_px: crate::snap::SNAP_TOLERANCE_CSS_PX,
            points: true,
            alignment: true,
            segments: true,
            page: true,
            guides: true,
            grid: false,
        }
    }
}

/// What a snap landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum SnapSource {
    /// A path anchor, or an oval's quadrant point. (`AnchorPoint` in
    /// Rust: wasm-bindgen's ABI traits already own an `Anchor` item.)
    #[serde(rename = "anchor")]
    AnchorPoint,
    /// A corner of a frame drawn from its box.
    Corner,
    /// An element's centre.
    Center,
    /// The page's corners, centre, edges or centre lines.
    Page,
    /// A ruler guide.
    Guide,
    /// A document gridline.
    Grid,
    /// A point the caller supplied (`extraPoints`).
    Extra,
}

/// Leave an element (or some of its anchors) out of the targets. With
/// `anchors` absent the whole element is ignored; with `anchors` given,
/// those anchors (flat indices, the `pathAnchors` numbering) are not
/// targets and neither are the element's segments, since a host editing
/// those anchors is previewing a path the engine has not seen yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapExclude {
    pub id: ElementId,
    #[serde(default)]
    pub anchors: Option<Vec<u32>>,
}

/// One point query. `point` is page-local pt on `page_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapPointQuery {
    pub page_id: PageId,
    pub point: [f32; 2],
    /// CSS px per pt at the current zoom. Absent ⇒ 1.
    #[serde(default)]
    pub camera_scale: Option<f32>,
    #[serde(default)]
    pub exclude: Vec<SnapExclude>,
    /// Points only the caller knows about, page-local — the anchors of a
    /// path still being drawn. Targets as points and as alignment lines.
    #[serde(default)]
    pub extra_points: Vec<[f32; 2]>,
}

/// A point target the query landed on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapPointHit {
    pub source: SnapSource,
    pub at: [f32; 2],
    #[serde(default)]
    pub element: Option<ElementId>,
    /// Flat anchor index when `source` is `anchor` on a path.
    #[serde(default)]
    pub anchor_index: Option<u32>,
}

/// An alignment line the query was pulled onto (one per axis).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapAxisHit {
    pub source: SnapSource,
    /// x for the vertical line, y for the horizontal one (page-local).
    pub position: f32,
    /// The element whose point the line runs through, for smart guides.
    #[serde(default)]
    pub element: Option<ElementId>,
    /// That point, so a host can draw the guide from it to the pointer.
    #[serde(default)]
    pub through: Option<[f32; 2]>,
}

/// The segment the query landed on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapSegmentHit {
    pub element: ElementId,
    /// Flat index of the segment's start / end anchor. Absent for an
    /// outline the element does not store as anchors (a box-drawn
    /// frame, an oval).
    #[serde(default)]
    pub seg_start: Option<u32>,
    #[serde(default)]
    pub seg_end: Option<u32>,
    pub t: f32,
}

/// `RequestSnapPoint` reply payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SnapPointResult {
    /// The point after snapping; the query's point when nothing snapped.
    pub point: [f32; 2],
    pub snapped: bool,
    #[serde(default)]
    pub point_target: Option<SnapPointHit>,
    #[serde(default)]
    pub x_target: Option<SnapAxisHit>,
    #[serde(default)]
    pub y_target: Option<SnapAxisHit>,
    #[serde(default)]
    pub segment_target: Option<SnapSegmentHit>,
    /// The guides to draw, same shape the translate gesture reports.
    pub lines: Vec<SnapLine>,
    /// The tolerance the query ran with, in pt.
    pub tolerance_pt: f32,
}

impl SnapPointResult {
    fn unsnapped(point: [f32; 2], tolerance_pt: f32) -> Self {
        Self {
            point,
            snapped: false,
            point_target: None,
            x_target: None,
            y_target: None,
            segment_target: None,
            lines: Vec::new(),
            tolerance_pt,
        }
    }
}

// ─── the index ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PointTarget {
    source: SnapSource,
    at: [f32; 2],
    anchor_index: Option<u32>,
}

#[derive(Debug, Clone)]
struct ElementTargets {
    id: ElementId,
    /// `[top, left, bottom, right]`, page-local: the outline's control
    /// hull, used to skip far elements.
    aabb: [f32; 4],
    /// The transformed geometric bounds, page-local — the box a moved
    /// frame's edges align with.
    box_aabb: [f32; 4],
    points: Vec<PointTarget>,
    /// The outline in PAGE space (an affine maps a cubic to a cubic, so
    /// transforming the anchors is exact).
    path: Vec<PathAnchor>,
    starts: Vec<usize>,
    open: Vec<bool>,
    /// The outline is the stored anchor table (segment indices mean
    /// something to the caller).
    path_is_anchors: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PageTargets {
    pub width: f32,
    pub height: f32,
    pub vertical_guides: Vec<f32>,
    pub horizontal_guides: Vec<f32>,
    elements: Vec<ElementTargets>,
}

/// Every page's snap targets for one document build.
#[derive(Debug, Clone, Default)]
pub(crate) struct SnapIndex {
    pub generation: u64,
    pub pages: HashMap<String, PageTargets>,
    /// Document grid spacing, pt: (horizontal division, vertical division).
    pub grid: Option<(f32, f32)>,
}

fn apply(m: [f32; 6], p: (f32, f32)) -> (f32, f32) {
    (
        m[0] * p.0 + m[2] * p.1 + m[4],
        m[1] * p.0 + m[3] * p.1 + m[5],
    )
}

/// One leaf's outline in its own space: the anchor table when it has
/// one, otherwise the shape its box draws.
struct LeafPath<'a> {
    id: ElementId,
    bounds: Bounds,
    item_transform: Option<[f32; 6]>,
    item_layer: Option<&'a str>,
    anchors: &'a [PathAnchor],
    starts: &'a [usize],
    open: &'a [bool],
    oval: bool,
}

fn leaves_of(spread: &Spread) -> Vec<LeafPath<'_>> {
    let mut out = Vec::new();
    for f in &spread.text_frames {
        if let Some(id) = f.self_id.as_ref() {
            out.push(LeafPath {
                id: ElementId::TextFrame(id.clone()),
                bounds: f.bounds,
                item_transform: f.item_transform,
                item_layer: f.item_layer.as_deref(),
                anchors: &f.anchors,
                starts: &f.subpath_starts,
                open: &f.subpath_open,
                oval: false,
            });
        }
    }
    for f in &spread.rectangles {
        if let Some(id) = f.self_id.as_ref() {
            out.push(LeafPath {
                id: ElementId::Rectangle(id.clone()),
                bounds: f.bounds,
                item_transform: f.item_transform,
                item_layer: f.item_layer.as_deref(),
                anchors: &f.anchors,
                starts: &f.subpath_starts,
                open: &f.subpath_open,
                oval: false,
            });
        }
    }
    for f in &spread.ovals {
        if let Some(id) = f.self_id.as_ref() {
            out.push(LeafPath {
                id: ElementId::Oval(id.clone()),
                bounds: f.bounds,
                item_transform: f.item_transform,
                item_layer: f.item_layer.as_deref(),
                anchors: &[],
                starts: &[],
                open: &[],
                oval: true,
            });
        }
    }
    for f in &spread.polygons {
        if let Some(id) = f.self_id.as_ref() {
            out.push(LeafPath {
                id: ElementId::Polygon(id.clone()),
                bounds: f.bounds,
                item_transform: f.item_transform,
                item_layer: f.item_layer.as_deref(),
                anchors: &f.anchors,
                starts: &f.subpath_starts,
                open: &f.subpath_open,
                oval: false,
            });
        }
    }
    for f in &spread.graphic_lines {
        if let Some(id) = f.self_id.as_ref() {
            out.push(LeafPath {
                id: ElementId::GraphicLine(id.clone()),
                bounds: f.bounds,
                item_transform: f.item_transform,
                item_layer: f.item_layer.as_deref(),
                anchors: &f.anchors,
                starts: &f.subpath_starts,
                open: &f.subpath_open,
                oval: false,
            });
        }
    }
    out
}

fn corner(p: (f32, f32)) -> PathAnchor {
    PathAnchor {
        anchor: p,
        left: p,
        right: p,
    }
}

/// The closed four-cubic ellipse inscribed in `b` (kappa 0.5523), the
/// shape the renderer draws for an oval.
fn ellipse_anchors(b: Bounds) -> Vec<PathAnchor> {
    const K: f32 = 0.552_284_8;
    let cx = (b.left + b.right) * 0.5;
    let cy = (b.top + b.bottom) * 0.5;
    let rx = (b.right - b.left) * 0.5;
    let ry = (b.bottom - b.top) * 0.5;
    vec![
        PathAnchor {
            anchor: (cx, b.top),
            left: (cx - K * rx, b.top),
            right: (cx + K * rx, b.top),
        },
        PathAnchor {
            anchor: (b.right, cy),
            left: (b.right, cy - K * ry),
            right: (b.right, cy + K * ry),
        },
        PathAnchor {
            anchor: (cx, b.bottom),
            left: (cx + K * rx, b.bottom),
            right: (cx - K * rx, b.bottom),
        },
        PathAnchor {
            anchor: (b.left, cy),
            left: (b.left, cy + K * ry),
            right: (b.left, cy - K * ry),
        },
    ]
}

fn element_targets(leaf: &LeafPath<'_>, origin: (f32, f32)) -> ElementTargets {
    let m = leaf
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let to_page = |p: (f32, f32)| {
        let (x, y) = apply(m, p);
        (x - origin.0, y - origin.1)
    };
    let (local, starts, open, path_is_anchors, source): (
        Vec<PathAnchor>,
        Vec<usize>,
        Vec<bool>,
        bool,
        SnapSource,
    ) = if leaf.oval {
        (
            ellipse_anchors(leaf.bounds),
            vec![0],
            vec![false],
            false,
            SnapSource::AnchorPoint,
        )
    } else if leaf.anchors.is_empty() {
        let b = leaf.bounds;
        (
            vec![
                corner((b.left, b.top)),
                corner((b.right, b.top)),
                corner((b.right, b.bottom)),
                corner((b.left, b.bottom)),
            ],
            vec![0],
            vec![false],
            false,
            SnapSource::Corner,
        )
    } else {
        (
            leaf.anchors.to_vec(),
            if leaf.starts.is_empty() {
                vec![0]
            } else {
                leaf.starts.to_vec()
            },
            leaf.open.to_vec(),
            true,
            SnapSource::AnchorPoint,
        )
    };
    let path: Vec<PathAnchor> = local
        .iter()
        .map(|a| PathAnchor {
            anchor: to_page(a.anchor),
            left: to_page(a.left),
            right: to_page(a.right),
        })
        .collect();
    let mut points: Vec<PointTarget> = path
        .iter()
        .enumerate()
        .map(|(i, a)| PointTarget {
            source,
            at: [a.anchor.0, a.anchor.1],
            anchor_index: path_is_anchors.then_some(i as u32),
        })
        .collect();
    let (mut t, mut l, mut b, mut r) = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    for a in &path {
        for p in [a.anchor, a.left, a.right] {
            l = l.min(p.0);
            r = r.max(p.0);
            t = t.min(p.1);
            b = b.max(p.1);
        }
    }
    let c = to_page((
        (leaf.bounds.left + leaf.bounds.right) * 0.5,
        (leaf.bounds.top + leaf.bounds.bottom) * 0.5,
    ));
    let bb = leaf.bounds;
    let corners = [
        to_page((bb.left, bb.top)),
        to_page((bb.right, bb.top)),
        to_page((bb.right, bb.bottom)),
        to_page((bb.left, bb.bottom)),
    ];
    let box_aabb = [
        corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min),
        corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
        corners
            .iter()
            .map(|p| p.1)
            .fold(f32::NEG_INFINITY, f32::max),
        corners
            .iter()
            .map(|p| p.0)
            .fold(f32::NEG_INFINITY, f32::max),
    ];
    points.push(PointTarget {
        source: SnapSource::Center,
        at: [c.0, c.1],
        anchor_index: None,
    });
    ElementTargets {
        id: leaf.id.clone(),
        aabb: [t, l, b, r],
        box_aabb,
        points,
        path,
        starts,
        open,
        path_is_anchors,
    }
}

/// Build every page's targets from the current scene + build.
pub(crate) fn build_index(
    scene: &paged_scene::Document,
    built: &BuiltDocument,
    generation: u64,
) -> SnapIndex {
    let designmap = &scene.designmap;
    let layer_renders = paged_scene::build_layer_render_map(designmap);
    let built_by_id: HashMap<&str, &paged_renderer::BuiltPage> =
        built.pages.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut pages: HashMap<String, PageTargets> = HashMap::new();
    for parsed in &scene.spreads {
        let spread = &parsed.spread;
        // This spread's pages, in spread coordinates.
        let spread_pages: Vec<(&str, &paged_renderer::BuiltPage)> = spread
            .pages
            .iter()
            .filter_map(|p| {
                let id = p.self_id.as_deref()?;
                built_by_id.get(id).map(|bp| (id, *bp))
            })
            .collect();
        if spread_pages.is_empty() {
            continue;
        }
        for (id, bp) in &spread_pages {
            pages
                .entry((*id).to_string())
                .or_insert_with(|| PageTargets {
                    width: bp.width_pt,
                    height: bp.height_pt,
                    ..Default::default()
                });
        }
        for g in &spread.guides {
            let idx = if g.page_index == 0 {
                0
            } else {
                ((g.page_index as usize) - 1).min(spread.pages.len().saturating_sub(1))
            };
            let Some(pid) = spread.pages.get(idx).and_then(|p| p.self_id.as_deref()) else {
                continue;
            };
            let Some(pt) = pages.get_mut(pid) else {
                continue;
            };
            match g.orientation {
                paged_model::GuideOrientation::Vertical => pt.vertical_guides.push(g.location),
                paged_model::GuideOrientation::Horizontal => pt.horizontal_guides.push(g.location),
            }
        }
        for leaf in leaves_of(spread) {
            if !paged_scene::lookup_layer_render_visible(&layer_renders, leaf.item_layer) {
                continue;
            }
            // The page whose rectangle holds the element's centre.
            let m = leaf
                .item_transform
                .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            let (cx, cy) = apply(
                m,
                (
                    (leaf.bounds.left + leaf.bounds.right) * 0.5,
                    (leaf.bounds.top + leaf.bounds.bottom) * 0.5,
                ),
            );
            let Some((pid, bp)) = spread_pages.iter().find(|(_, bp)| {
                cx >= bp.spread_origin.0
                    && cx <= bp.spread_origin.0 + bp.width_pt
                    && cy >= bp.spread_origin.1
                    && cy <= bp.spread_origin.1 + bp.height_pt
            }) else {
                continue;
            };
            let targets = element_targets(&leaf, bp.spread_origin);
            if let Some(pt) = pages.get_mut(*pid) {
                pt.elements.push(targets);
            }
        }
    }
    let gp = &scene.designmap.grid_preference;
    let grid = match (
        gp.horizontal_gridline_division,
        gp.vertical_gridline_division,
    ) {
        (Some(h), Some(v)) if h > 0.0 && v > 0.0 => Some((h, v)),
        (Some(h), None) if h > 0.0 => Some((h, h)),
        (None, Some(v)) if v > 0.0 => Some((v, v)),
        _ => None,
    };
    SnapIndex {
        generation,
        pages,
        grid,
    }
}

// ─── the resolver ───────────────────────────────────────────────────

/// Candidate x / y lines for one page, each tagged with where it came
/// from. Shared with the gestures so a move and a pen click see the same
/// lines.
#[derive(Debug, Clone)]
pub(crate) struct AxisLine {
    pub position: f32,
    pub source: SnapSource,
    pub element: Option<ElementId>,
    pub through: Option<[f32; 2]>,
}

fn excluded_whole(exclude: &[SnapExclude], id: &ElementId) -> bool {
    exclude.iter().any(|e| &e.id == id && e.anchors.is_none())
}

fn excluded_anchor(exclude: &[SnapExclude], id: &ElementId, index: Option<u32>) -> bool {
    exclude.iter().any(|e| {
        &e.id == id
            && match (&e.anchors, index) {
                (None, _) => true,
                (Some(list), Some(i)) => list.contains(&i),
                (Some(_), None) => false,
            }
    })
}

fn editing(exclude: &[SnapExclude], id: &ElementId) -> bool {
    exclude.iter().any(|e| &e.id == id)
}

/// The page-and-guide-and-grid lines for one axis (no element lines).
pub(crate) fn static_lines(
    page: &PageTargets,
    grid: Option<(f32, f32)>,
    settings: &SnapSettings,
    axis: SnapAxis,
    near: f32,
) -> Vec<AxisLine> {
    let mut out = Vec::new();
    let line = |position: f32, source: SnapSource| AxisLine {
        position,
        source,
        element: None,
        through: None,
    };
    let extent = match axis {
        SnapAxis::X => page.width,
        SnapAxis::Y => page.height,
    };
    if settings.page {
        for p in [0.0, extent * 0.5, extent] {
            out.push(line(p, SnapSource::Page));
        }
    }
    if settings.guides {
        let guides = match axis {
            SnapAxis::X => &page.vertical_guides,
            SnapAxis::Y => &page.horizontal_guides,
        };
        for &g in guides {
            out.push(line(g, SnapSource::Guide));
        }
    }
    if settings.grid {
        if let Some((h, v)) = grid {
            // A vertical gridline (x) is spaced by the VERTICAL division.
            let step = match axis {
                SnapAxis::X => v,
                SnapAxis::Y => h,
            };
            let k = (near / step).round();
            out.push(line(k * step, SnapSource::Grid));
        }
    }
    out
}

fn nearest_line(lines: &[AxisLine], value: f32, tolerance: f32) -> Option<&AxisLine> {
    let mut best: Option<(&AxisLine, f32)> = None;
    for l in lines {
        let d = (l.position - value).abs();
        if d <= tolerance && best.map_or(true, |(_, bd)| d < bd) {
            best = Some((l, d));
        }
    }
    best.map(|(l, _)| l)
}

/// Resolve one point query against the index.
pub(crate) fn resolve(
    index: &SnapIndex,
    settings: &SnapSettings,
    query: &SnapPointQuery,
) -> SnapPointResult {
    let scale = query.camera_scale.unwrap_or(1.0).max(1e-3);
    let tolerance = settings.tolerance_px.max(0.0) / scale;
    let p = query.point;
    if !settings.enabled
        || tolerance.is_nan()
        || tolerance <= 0.0
        || !p[0].is_finite()
        || !p[1].is_finite()
    {
        return SnapPointResult::unsnapped(p, tolerance);
    }
    let Some(page) = index.pages.get(query.page_id.as_str()) else {
        return SnapPointResult::unsnapped(p, tolerance);
    };
    let near = |a: [f32; 4]| {
        p[0] >= a[1] - tolerance
            && p[0] <= a[3] + tolerance
            && p[1] >= a[0] - tolerance
            && p[1] <= a[2] + tolerance
    };

    // 1. points — the nearest within tolerance wins outright.
    let mut best: Option<(SnapPointHit, f32)> = None;
    let mut consider = |hit: SnapPointHit| {
        let d = (hit.at[0] - p[0]).hypot(hit.at[1] - p[1]);
        if d <= tolerance && best.as_ref().map_or(true, |(_, bd)| d < *bd) {
            best = Some((hit, d));
        }
    };
    if settings.page {
        let (w, h) = (page.width, page.height);
        for at in [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h], [w * 0.5, h * 0.5]] {
            consider(SnapPointHit {
                source: SnapSource::Page,
                at,
                element: None,
                anchor_index: None,
            });
        }
    }
    for &at in &query.extra_points {
        consider(SnapPointHit {
            source: SnapSource::Extra,
            at,
            element: None,
            anchor_index: None,
        });
    }
    if settings.points {
        for el in &page.elements {
            if !near(el.aabb) || excluded_whole(&query.exclude, &el.id) {
                continue;
            }
            for pt in &el.points {
                if excluded_anchor(&query.exclude, &el.id, pt.anchor_index) {
                    continue;
                }
                consider(SnapPointHit {
                    source: pt.source,
                    at: pt.at,
                    element: Some(el.id.clone()),
                    anchor_index: pt.anchor_index,
                });
            }
        }
    }
    if let Some((hit, _)) = best {
        let at = hit.at;
        return SnapPointResult {
            point: at,
            snapped: true,
            point_target: Some(hit),
            x_target: None,
            y_target: None,
            segment_target: None,
            lines: Vec::new(),
            tolerance_pt: tolerance,
        };
    }

    // 2. alignment lines, each axis on its own.
    let mut xs = static_lines(page, index.grid, settings, SnapAxis::X, p[0]);
    let mut ys = static_lines(page, index.grid, settings, SnapAxis::Y, p[1]);
    if settings.alignment {
        let mut push = |at: [f32; 2], element: Option<ElementId>, source: SnapSource| {
            if (at[0] - p[0]).abs() <= tolerance {
                xs.push(AxisLine {
                    position: at[0],
                    source,
                    element: element.clone(),
                    through: Some(at),
                });
            }
            if (at[1] - p[1]).abs() <= tolerance {
                ys.push(AxisLine {
                    position: at[1],
                    source,
                    element,
                    through: Some(at),
                });
            }
        };
        for &at in &query.extra_points {
            push(at, None, SnapSource::Extra);
        }
        for el in &page.elements {
            if excluded_whole(&query.exclude, &el.id) {
                continue;
            }
            for pt in &el.points {
                if excluded_anchor(&query.exclude, &el.id, pt.anchor_index) {
                    continue;
                }
                push(pt.at, Some(el.id.clone()), pt.source);
            }
        }
    }
    let bx = nearest_line(&xs, p[0], tolerance);
    let by = nearest_line(&ys, p[1], tolerance);
    if bx.is_some() || by.is_some() {
        let x = bx.map_or(p[0], |l| l.position);
        let y = by.map_or(p[1], |l| l.position);
        let hit = |l: &AxisLine| SnapAxisHit {
            source: l.source,
            position: l.position,
            element: l.element.clone(),
            through: l.through,
        };
        let mut lines = Vec::new();
        if let Some(l) = bx {
            lines.push(SnapLine {
                axis: SnapAxis::X,
                position: l.position,
                page_id: query.page_id.clone(),
            });
        }
        if let Some(l) = by {
            lines.push(SnapLine {
                axis: SnapAxis::Y,
                position: l.position,
                page_id: query.page_id.clone(),
            });
        }
        return SnapPointResult {
            point: [x, y],
            snapped: true,
            point_target: None,
            x_target: bx.map(hit),
            y_target: by.map(hit),
            segment_target: None,
            lines,
            tolerance_pt: tolerance,
        };
    }

    // 3. segments — the nearest point on the nearest outline.
    if settings.segments {
        let mut bs: Option<(SnapSegmentHit, [f32; 2], f32)> = None;
        for el in &page.elements {
            if !near(el.aabb) || editing(&query.exclude, &el.id) {
                continue;
            }
            let Some(hit) = paged_mutate::kurbo_kernel::nearest_point_on_path(
                &el.path,
                &el.starts,
                &el.open,
                (p[0], p[1]),
            ) else {
                continue;
            };
            if hit.distance <= tolerance && bs.as_ref().map_or(true, |(_, _, d)| hit.distance < *d)
            {
                bs = Some((
                    SnapSegmentHit {
                        element: el.id.clone(),
                        seg_start: el.path_is_anchors.then_some(hit.seg_start as u32),
                        seg_end: el.path_is_anchors.then_some(hit.seg_end as u32),
                        t: hit.t,
                    },
                    [hit.point.0, hit.point.1],
                    hit.distance,
                ));
            }
        }
        if let Some((seg, at, _)) = bs {
            return SnapPointResult {
                point: at,
                snapped: true,
                point_target: None,
                x_target: None,
                y_target: None,
                segment_target: Some(seg),
                lines: Vec::new(),
                tolerance_pt: tolerance,
            };
        }
    }
    SnapPointResult::unsnapped(p, tolerance)
}

/// Every visible leaf's box on every page, the sibling set a translate
/// or resize gesture aligns edges with. All five leaf kinds, not only
/// text frames and rectangles as before v67.
pub(crate) fn frame_rects(index: &SnapIndex) -> Vec<crate::snap::FrameRect> {
    let mut out = Vec::new();
    for (page_id, page) in &index.pages {
        for el in &page.elements {
            out.push(crate::snap::FrameRect {
                element_id: el.id.clone(),
                page_id: PageId(page_id.clone()),
                aabb: el.box_aabb,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_with(elements: Vec<ElementTargets>) -> SnapIndex {
        let mut pages = HashMap::new();
        pages.insert(
            "p1".to_string(),
            PageTargets {
                width: 600.0,
                height: 800.0,
                vertical_guides: vec![100.0],
                horizontal_guides: vec![],
                elements,
            },
        );
        SnapIndex {
            generation: 1,
            pages,
            grid: Some((12.0, 12.0)),
        }
    }

    fn rect(id: &str, b: Bounds) -> ElementTargets {
        element_targets(
            &LeafPath {
                id: ElementId::Rectangle(id.to_string()),
                bounds: b,
                item_transform: None,
                item_layer: None,
                anchors: &[],
                starts: &[],
                open: &[],
                oval: false,
            },
            (0.0, 0.0),
        )
    }

    fn q(x: f32, y: f32) -> SnapPointQuery {
        SnapPointQuery {
            page_id: PageId("p1".to_string()),
            point: [x, y],
            camera_scale: Some(1.0),
            exclude: vec![],
            extra_points: vec![],
        }
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
    fn a_point_wins_outright() {
        let idx = page_with(vec![rect("r", b(200.0, 200.0, 300.0, 300.0))]);
        let r = resolve(&idx, &SnapSettings::default(), &q(202.0, 197.0));
        assert_eq!(r.point, [200.0, 200.0]);
        let hit = r.point_target.expect("a corner");
        assert_eq!(hit.source, SnapSource::Corner);
        assert_eq!(hit.element, Some(ElementId::Rectangle("r".into())));
    }

    #[test]
    fn axes_snap_independently_to_alignment_lines() {
        let idx = page_with(vec![rect("r", b(200.0, 200.0, 300.0, 300.0))]);
        // x near the guide at 100, y near the rect's top edge line 200.
        let r = resolve(&idx, &SnapSettings::default(), &q(102.0, 203.0));
        assert_eq!(r.point, [100.0, 200.0]);
        assert_eq!(r.x_target.as_ref().unwrap().source, SnapSource::Guide);
        let y = r.y_target.unwrap();
        assert_eq!(y.source, SnapSource::Corner);
        assert_eq!(y.through, Some([200.0, 200.0]));
        assert_eq!(r.lines.len(), 2);
    }

    #[test]
    fn a_segment_catches_what_no_point_or_line_does() {
        let idx = page_with(vec![rect("r", b(200.0, 200.0, 300.0, 300.0))]);
        // With alignment on, the corners' y line (300) wins first, as it
        // should; the segment is what is left when no line is near.
        let s = SnapSettings {
            alignment: false,
            ..SnapSettings::default()
        };
        let r = resolve(&idx, &s, &q(252.0, 302.5));
        assert!((r.point[0] - 252.0).abs() < 1e-3 && (r.point[1] - 300.0).abs() < 1e-3);
        assert!(r.segment_target.is_some());
        assert!(r.x_target.is_none() && r.y_target.is_none());
    }

    #[test]
    fn outside_tolerance_nothing_moves() {
        let idx = page_with(vec![rect("r", b(200.0, 200.0, 300.0, 300.0))]);
        let r = resolve(&idx, &SnapSettings::default(), &q(150.0, 150.0));
        assert!(!r.snapped);
        assert_eq!(r.point, [150.0, 150.0]);
    }

    #[test]
    fn tolerance_is_screen_px() {
        let idx = page_with(vec![rect("r", b(200.0, 200.0, 300.0, 300.0))]);
        let mut query = q(206.0, 206.0); // 8.5 pt from the corner
        query.camera_scale = Some(1.0);
        assert!(!resolve(&idx, &SnapSettings::default(), &query).snapped);
        query.camera_scale = Some(0.25); // 4 px = 16 pt at 25 %
        assert_eq!(
            resolve(&idx, &SnapSettings::default(), &query).point,
            [200.0, 200.0]
        );
    }

    #[test]
    fn excluded_anchors_are_not_targets_and_their_outline_is_not_either() {
        let idx = page_with(vec![rect("r", b(200.0, 200.0, 300.0, 300.0))]);
        let mut query = q(252.0, 302.5);
        query.exclude = vec![SnapExclude {
            id: ElementId::Rectangle("r".into()),
            anchors: Some(vec![]),
        }];
        let r = resolve(&idx, &SnapSettings::default(), &query);
        assert!(r.segment_target.is_none());
        let mut whole = q(202.0, 197.0);
        whole.exclude = vec![SnapExclude {
            id: ElementId::Rectangle("r".into()),
            anchors: None,
        }];
        assert!(resolve(&idx, &SnapSettings::default(), &whole)
            .point_target
            .is_none());
    }

    #[test]
    fn the_grid_is_off_until_asked_for() {
        let idx = page_with(vec![]);
        // Clear of the page's edges and centre lines and of the guide.
        let r = resolve(&idx, &SnapSettings::default(), &q(49.5, 410.5));
        assert!(!r.snapped);
        let s = SnapSettings {
            grid: true,
            ..SnapSettings::default()
        };
        let r = resolve(&idx, &s, &q(49.5, 410.5));
        assert_eq!(r.point, [48.0, 408.0]);
        assert_eq!(r.x_target.unwrap().source, SnapSource::Grid);
    }

    #[test]
    fn extra_points_are_targets() {
        let idx = page_with(vec![]);
        let mut query = q(401.0, 501.0);
        query.extra_points = vec![[400.0, 500.0]];
        let r = resolve(&idx, &SnapSettings::default(), &query);
        assert_eq!(r.point_target.unwrap().source, SnapSource::Extra);
    }

    #[test]
    fn disabled_answers_the_point_it_was_given() {
        let idx = page_with(vec![rect("r", b(200.0, 200.0, 300.0, 300.0))]);
        let s = SnapSettings {
            enabled: false,
            ..SnapSettings::default()
        };
        assert!(!resolve(&idx, &s, &q(201.0, 201.0)).snapped);
    }

    #[test]
    fn an_oval_offers_its_quadrant_points_and_its_curve() {
        let leaf = LeafPath {
            id: ElementId::Oval("o".into()),
            bounds: b(0.0, 0.0, 100.0, 200.0),
            item_transform: Some([1.0, 0.0, 0.0, 1.0, 300.0, 300.0]),
            item_layer: None,
            anchors: &[],
            starts: &[],
            open: &[],
            oval: true,
        };
        let idx = page_with(vec![element_targets(&leaf, (0.0, 0.0))]);
        let r = resolve(&idx, &SnapSettings::default(), &q(401.0, 299.0));
        assert_eq!(r.point, [400.0, 300.0]);
        assert_eq!(r.point_target.unwrap().source, SnapSource::AnchorPoint);
    }
}
