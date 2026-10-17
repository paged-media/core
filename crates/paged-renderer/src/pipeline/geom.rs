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

//! Geometry + style helpers: page-margin box, matrix/transform, wrap-rect collection, style-id predicates, path hashing. Extracted from pipeline/mod.rs (1.6b).

use super::*;
use std::collections::HashMap;

use paged_compose::{DisplayList, GlyphCacheKey, GlyphOutliner, Transform};
use paged_model::PathAnchor;
use paged_scene::Document;

/// Resolve the host page's `<MarginPreference>` margin box into a
/// page-local pt rectangle for the anchored `PageMargins` reference
/// point. The margins live on the parsed `Spread` as a side map
/// (`page_margins`) keyed by the page's `Self` id (W0.6); `BuiltPage::id`
/// carries that same id. Page `Self` ids are document-unique, so a flat
/// scan across spreads finds the one owning this page. Margins inset the
/// page rectangle, so the box is `[left, top, width-right, height-bottom]`
/// in the page's own (0,0)-top-left coordinate frame. Returns `None` when
/// the page declared no margins (the reference then degenerates to the
/// page edge).
pub(super) fn resolve_page_margin_box(
    document: &Document,
    page: &BuiltPage,
) -> Option<anchored::PageMarginBox> {
    let page_self = page.id.0.as_str();
    if page_self.is_empty() {
        return None;
    }
    let m = document
        .spreads
        .iter()
        .find_map(|s| s.spread.page_margins.get(page_self))?;
    Some(anchored::PageMarginBox {
        left: m.left,
        top: m.top,
        right: (page.width_pt - m.right).max(m.left),
        bottom: (page.height_pt - m.bottom).max(m.top),
    })
}

/// Wraps a page's bounds for centre-point routing + its master
/// reference for master-spread application + its position in the
/// document so the master pass can read back per-page state
/// (MasterPageTransform).
pub(super) struct PageGeom {
    pub(super) bounds_in_spread: paged_model::Bounds,
    pub(super) applied_master: Option<String>,
    pub(super) host_spread_idx: usize,
    pub(super) local_page_idx: usize,
}

/// Local mirror of `paged_compose::text::get_or_intern_glyph_outline`,
/// which is private. Same caching key (font_id × glyph_id) so glyphs
/// emitted via the body-text path and the text-on-path path share
/// outlines.
pub(super) fn list_get_or_intern_glyph_outline<O: GlyphOutliner>(
    font_id: u32,
    glyph_id: u32,
    outliner: &O,
    list: &mut DisplayList,
) -> Option<paged_compose::PathId> {
    let key = GlyphCacheKey { font_id, glyph_id }.to_u64();
    if let Some(existing) = list.paths.find_by_key(key) {
        return Some(existing);
    }
    let outline = outliner.outline(glyph_id)?;
    let (id, _) = list.paths.intern(key, outline);
    Some(id)
}

/// Cheap content-derived cache key for polygons that don't carry a
/// `Self` id (synthetic / minified IDMLs). FNV-1a of the
/// concatenated anchor coordinates.
pub(crate) fn path_signature(anchors: &[PathAnchor]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for a in anchors {
        for v in [
            a.anchor.0, a.anchor.1, a.left.0, a.left.1, a.right.0, a.right.1,
        ] {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
    }
    h
}

pub(crate) fn fnv_1a_u64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Decode the UTF-8 character starting at byte offset `i` in `bytes`.
/// Returns `None` when `i` is past the end or doesn't sit on a UTF-8
/// boundary. Used by the optical-margin pass to look up the
/// leftmost / rightmost glyph's source codepoint by cluster, since
/// `PositionedGlyph::cluster` is a byte offset into the paragraph's
/// concatenated source text.
pub(super) fn char_at_byte(bytes: &[u8], i: usize) -> Option<char> {
    if i >= bytes.len() {
        return None;
    }
    // Walk forward up to 4 bytes — the maximum UTF-8 sequence
    // length — and decode lazily via std::str::from_utf8.
    let end = (i + 4).min(bytes.len());
    let slice = &bytes[i..end];
    std::str::from_utf8(slice)
        .ok()
        .and_then(|s| s.chars().next())
        .or_else(|| {
            // If the 4-byte window straddled an invalid boundary
            // (rare — clusters can land on byte-start of any
            // codepoint), fall back to a slower scan from byte 0.
            std::str::from_utf8(&bytes[..end])
                .ok()
                .and_then(|s| s[i..].chars().next())
        })
}

/// Apply a 6-element IDML affine `[a b c d e f]` to `(x, y)`.
/// Per IDML spec §10.3.3 the matrix maps inner→parent coords:
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
pub(super) fn apply_matrix(m: &[f32; 6], x: f32, y: f32) -> (f32, f32) {
    let [a, b, c, d, e, f] = *m;
    (a * x + c * y + e, b * x + d * y + f)
}

/// Transform an axis-aligned `Bounds` by an IDML affine and return
/// the AABB of the result. Identity (`None`) is the no-op.
/// For pure translation (the common Page.ItemTransform case) this
/// preserves width/height; for the 90° page rotations the spec
/// allows on whole spreads, the AABB swaps width/height — the right
/// behaviour for routing + canvas sizing.
/// W1.9 — the LINEAR part (rotation / scale) of a spread-level
/// `<Spread ItemTransform>`, with the translation dropped (it cancels
/// against the spread-inner page origin in `frame_outer_transform`).
/// Returns `Transform::IDENTITY` when the transform is absent or is a
/// pure translation (`[1 0 0 1 tx ty]`) — the overwhelmingly common
/// case — so the per-page composition stays byte-identical to the
/// pre-W1.9 path. Applied *about the page origin*, so only the 2×2
/// linear block matters here.
pub(super) fn spread_linear_transform(m: Option<[f32; 6]>) -> Transform {
    match m {
        Some([a, b, c, d, _, _]) => {
            let is_identity_linear = (a - 1.0).abs() < 1e-6
                && b.abs() < 1e-6
                && c.abs() < 1e-6
                && (d - 1.0).abs() < 1e-6;
            if is_identity_linear {
                Transform::IDENTITY
            } else {
                Transform([a, b, c, d, 0.0, 0.0])
            }
        }
        None => Transform::IDENTITY,
    }
}

/// Which page of a master spread the body page `local` (an index into
/// `body`) takes: the master page on the same side of the spine, counted
/// out from it. `body` and `master` are page bounds in their own spread's
/// coordinates, where the spine is x = 0 — InDesign lays a spread's pages
/// out around it, left-hand pages at negative x.
///
/// The side, not the index: a spread holding one page right of the spine
/// (`BindingLocation="0"`) holds a right-hand page, and InDesign gives it
/// the master's right page; as index 0 it used to take the left one. A
/// master with no page on that side (a one-page master under a verso)
/// falls back to the index, and so does a page ON the spine — a
/// single-sided document's, centred on x = 0 — which belongs to neither
/// side: InDesign 20.0.1 prints a two-page master's LEFT page on it
/// (tools/indesign-export/master-items-probe, whose single-sided page
/// keeps the default facing master).
pub(super) fn master_page_index(
    body: &[paged_model::Bounds],
    local: usize,
    master: &[paged_model::Bounds],
) -> usize {
    let by_index = local.min(master.len().saturating_sub(1));
    let Some(page) = body.get(local) else {
        return by_index;
    };
    if page.left < 0.0 && page.right > 0.0 {
        return by_index;
    }
    let centre = |b: &paged_model::Bounds| (b.left + b.right) * 0.5;
    let right_of_spine = |b: &paged_model::Bounds| centre(b) >= 0.0;
    let side = right_of_spine(page);
    // How many pages on this side sit nearer the spine than this one.
    let ordinal = body
        .iter()
        .filter(|b| right_of_spine(b) == side && centre(b).abs() < centre(page).abs())
        .count();
    let mut same_side: Vec<usize> = (0..master.len())
        .filter(|&i| right_of_spine(&master[i]) == side)
        .collect();
    if same_side.is_empty() {
        return by_index;
    }
    same_side.sort_by(|&a, &b| {
        centre(&master[a])
            .abs()
            .total_cmp(&centre(&master[b]).abs())
    });
    same_side[ordinal.min(same_side.len() - 1)]
}

pub(crate) fn transform_bounds(b: paged_model::Bounds, m: Option<[f32; 6]>) -> paged_model::Bounds {
    let Some(m) = m else { return b };
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
    paged_model::Bounds {
        top: min_y,
        left: min_x,
        bottom: max_y,
        right: max_x,
    }
}

/// A text-wrap obstacle: AABB bounds plus the four corner points of
/// the (possibly rotated) source rectangle in spread coords. The
/// AABB drives fast vertical/horizontal rejection and the simple
/// side-shrink heuristic; the polygon corners drive per-line carve
/// against rotated obstacles so a rotated rect's wrap follows its
/// actual angled edges instead of its much wider unrotated AABB.
#[derive(Debug, Clone)]
pub(super) struct WrapShape {
    pub(super) bounds: paged_model::Bounds,
    pub(super) corners: [(f32, f32); 4],
    pub(super) kind: WrapKind,
    /// The side of the obstacle a line may use, resolved for its page.
    pub(super) side: WrapSide,
    /// A contour wrap's outline; `None` for a bounding-box wrap.
    pub(super) contour: Option<std::sync::Arc<WrapContour>>,
}

/// A contour wrap's zone: every point within `reach` of the outline
/// (the outline grown with round joins), as InDesign draws it. Asked of
/// InDesign 20.0.1 (`text-wrap-sides` pages 12 and 13, ADR 151): the
/// reach is the wrap's top offset plus the outer half of a visible
/// stroke, and a triangle's corner reaches exactly that far.
#[derive(Debug)]
pub(super) struct WrapContour {
    /// The outline in spread coords, sampled at most `CONTOUR_STEP_PT`
    /// apart.
    pub(super) points: Vec<(f32, f32)>,
    pub(super) reach: f32,
}

/// The densest a contour's outline is sampled, in pt.
const CONTOUR_STEP_PT: f32 = 0.25;

/// The side of a wrap obstacle a line may use (`TextWrapSide`), with
/// the spine sides resolved for the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WrapSide {
    Both,
    /// Text stands left of the obstacle only.
    Left,
    /// Text stands right of the obstacle only.
    Right,
    /// Whichever side has more room on the line.
    Largest,
}

impl WrapSide {
    /// Asked of InDesign 20.0.1 (`text-wrap-sides` pages 2–9): a verso's
    /// spine is on its right, so text towards it stands right of the
    /// obstacle there and left of it on a recto.
    pub(super) fn resolve(side: Option<paged_model::TextWrapSide>, verso: bool) -> Self {
        use paged_model::TextWrapSide as S;
        match side {
            None | Some(S::BothSides) => WrapSide::Both,
            Some(S::LeftSide) => WrapSide::Left,
            Some(S::RightSide) => WrapSide::Right,
            Some(S::LargestArea) => WrapSide::Largest,
            Some(S::SideTowardsSpine) if verso => WrapSide::Right,
            Some(S::SideTowardsSpine) => WrapSide::Left,
            Some(S::SideAwayFromSpine) if verso => WrapSide::Left,
            Some(S::SideAwayFromSpine) => WrapSide::Right,
        }
    }

    /// The span a line in the text area `left..right` loses to an
    /// obstacle whose zone on the line is `wl..wr`.
    pub(super) fn exclusion(self, wl: f32, wr: f32, left: f32, right: f32) -> (f32, f32) {
        let text_left = (wl, f32::INFINITY);
        let text_right = (f32::NEG_INFINITY, wr);
        match self {
            WrapSide::Both => (wl, wr),
            WrapSide::Left => text_left,
            WrapSide::Right => text_right,
            WrapSide::Largest if wl - left >= right - wr => text_left,
            WrapSide::Largest => text_right,
        }
    }
}

/// What a wrap obstacle's zone is built from: its geometry and the
/// stroke a contour reaches past.
pub(super) struct WrapSource<'a> {
    pub(super) bounds: paged_model::Bounds,
    pub(super) item_transform: Option<[f32; 6]>,
    pub(super) anchors: &'a [PathAnchor],
    pub(super) subpath_starts: &'a [usize],
    pub(super) subpath_open: &'a [bool],
    pub(super) oval: bool,
    pub(super) stroke_color: Option<&'a str>,
    pub(super) stroke_weight: Option<f32>,
}

impl WrapSource<'_> {
    /// The zone `wrap` keeps text out of around this item, on a page
    /// that is a verso or not; `None` for no active wrap.
    pub(super) fn shape(&self, wrap: paged_model::TextWrap, verso: bool) -> Option<WrapShape> {
        use paged_model::TextWrapMode as M;
        if !wrap.mode.is_active() {
            return None;
        }
        // Bounding box and contour narrow the lines; jump object and
        // next column move them instead (`WrapKind`).
        let kind = match wrap.mode {
            M::BoundingBoxTextWrap | M::ContourTextWrap => WrapKind::Around,
            M::JumpObjectTextWrap => WrapKind::JumpObject,
            M::NextColumnTextWrap => WrapKind::NextColumn,
            _ => return None,
        };
        let side = WrapSide::resolve(wrap.side, verso);
        if wrap.mode == M::ContourTextWrap {
            if let Some(contour) = self.contour(wrap) {
                return Some(WrapShape::from_contour(contour, kind, side));
            }
        }
        Some(WrapShape {
            kind,
            side,
            ..WrapShape::from_inner(self.bounds, self.item_transform, wrap.offsets)
        })
    }

    /// The item's outline in spread coords, densely sampled, with the
    /// wrap's reach: InDesign uses only the top offset for a contour.
    fn contour(&self, wrap: paged_model::TextWrap) -> Option<WrapContour> {
        let path = if self.oval {
            super::shapes::ellipse_outline_path(Rect {
                x: self.bounds.left,
                y: self.bounds.top,
                w: self.bounds.width(),
                h: self.bounds.height(),
            })
        } else if self.anchors.is_empty() {
            super::shapes::axis_rect_path(Rect {
                x: self.bounds.left,
                y: self.bounds.top,
                w: self.bounds.width(),
                h: self.bounds.height(),
            })
        } else {
            super::text_path::polygon_path_from_anchors_with_open(
                self.anchors,
                self.subpath_starts,
                self.subpath_open,
            )
        };
        let map = |(x, y): (f32, f32)| match self.item_transform {
            Some(m) => apply_matrix(&m, x, y),
            None => (x, y),
        };
        let mut points = Vec::new();
        for line in super::stroke_geom::flatten_path(&path, 32) {
            let pts: Vec<(f32, f32)> = line.points.iter().copied().map(map).collect();
            let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else {
                continue;
            };
            let closing = line.closed.then_some((last, first));
            let edges = pts.windows(2).map(|w| (w[0], w[1])).chain(closing);
            points.push(first);
            for (a, b) in edges {
                let n = ((b.0 - a.0).hypot(b.1 - a.1) / CONTOUR_STEP_PT)
                    .ceil()
                    .max(1.0);
                for k in 1..=n as usize {
                    let t = k as f32 / n;
                    points.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
                }
            }
        }
        if points.is_empty() {
            return None;
        }
        let stroke = self.stroke_weight.unwrap_or(0.0);
        let stroke_reach = if frame_stroke_is_visible(self.stroke_color, stroke) {
            stroke * 0.5
        } else {
            0.0
        };
        Some(WrapContour {
            points,
            reach: (wrap.offsets[0] + stroke_reach).max(0.0),
        })
    }
}

/// How text treats a wrap obstacle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WrapKind {
    /// Bounding box and contour: lines are narrowed around it.
    Around,
    /// `JumpObjectTextWrap`: no line sits beside it; a line whose slug
    /// meets it moves down a leading at a time until it clears.
    JumpObject,
    /// `NextColumnTextWrap`: the line that meets it, and the rest of
    /// the story, continue in the next column.
    NextColumn,
}

impl WrapShape {
    /// Build from an inner-coord `Bounds`, an optional ItemTransform,
    /// and per-side wrap offsets `[top, left, bottom, right]`. The
    /// offsets inflate the unrotated source rect *before* the
    /// transform applies so the polygon stays aligned with the host's
    /// rotation (offset is in inner-coord points, same as InDesign).
    pub(super) fn from_inner(
        b: paged_model::Bounds,
        m: Option<[f32; 6]>,
        offsets: [f32; 4],
    ) -> Self {
        let inner = paged_model::Bounds {
            top: b.top - offsets[0],
            left: b.left - offsets[1],
            bottom: b.bottom + offsets[2],
            right: b.right + offsets[3],
        };
        let corners = match m {
            Some(m) => [
                apply_matrix(&m, inner.left, inner.top),
                apply_matrix(&m, inner.right, inner.top),
                apply_matrix(&m, inner.right, inner.bottom),
                apply_matrix(&m, inner.left, inner.bottom),
            ],
            None => [
                (inner.left, inner.top),
                (inner.right, inner.top),
                (inner.right, inner.bottom),
                (inner.left, inner.bottom),
            ],
        };
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
        let bounds = paged_model::Bounds {
            top: min_y,
            left: min_x,
            bottom: max_y,
            right: max_x,
        };
        Self {
            bounds,
            corners,
            kind: WrapKind::Around,
            side: WrapSide::Both,
            contour: None,
        }
    }

    /// A contour wrap's zone: its bounds are the outline's grown by the
    /// reach.
    fn from_contour(contour: WrapContour, kind: WrapKind, side: WrapSide) -> Self {
        let r = contour.reach;
        let (mut l, mut t, mut rt, mut b) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for &(x, y) in &contour.points {
            l = l.min(x);
            rt = rt.max(x);
            t = t.min(y);
            b = b.max(y);
        }
        let bounds = paged_model::Bounds {
            top: t - r,
            left: l - r,
            bottom: b + r,
            right: rt + r,
        };
        Self {
            bounds,
            corners: [
                (bounds.left, bounds.top),
                (bounds.right, bounds.top),
                (bounds.right, bounds.bottom),
                (bounds.left, bounds.bottom),
            ],
            kind,
            side,
            contour: Some(std::sync::Arc::new(contour)),
        }
    }

    /// Return the polygon's projected x-extent within the horizontal
    /// strip `[band_top, band_bottom]` (spread y). Returns `None` if
    /// the polygon doesn't intersect the strip vertically. The result
    /// is the (min_x, max_x) range over all polygon points whose y
    /// lies inside the strip plus all polygon-edge crossings of the
    /// strip's top and bottom horizontal lines. This handles both
    /// upright AABBs (where corners themselves bound the answer) and
    /// rotated parallelograms (where edges crossing the strip yield
    /// the carve.
    pub(super) fn x_extent_in_band(&self, band_top: f32, band_bottom: f32) -> Option<(f32, f32)> {
        if self.bounds.bottom <= band_top || self.bounds.top >= band_bottom {
            return None;
        }
        if let Some(c) = &self.contour {
            // The grown outline across the band: each outline point
            // reaches `reach` round it, so one a distance `dy` above or
            // below the band still covers sqrt(reach² − dy²) either side.
            let mut span: Option<(f32, f32)> = None;
            for &(x, y) in &c.points {
                let dy = if y < band_top {
                    band_top - y
                } else if y > band_bottom {
                    y - band_bottom
                } else {
                    0.0
                };
                if dy > c.reach {
                    continue;
                }
                let half = (c.reach * c.reach - dy * dy).max(0.0).sqrt();
                span = Some(match span {
                    Some((a, b)) => (a.min(x - half), b.max(x + half)),
                    None => (x - half, x + half),
                });
            }
            return span;
        }
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut visit = |x: f32| {
            if x < min_x {
                min_x = x;
            }
            if x > max_x {
                max_x = x;
            }
        };
        // Corners that lie inside the strip.
        for (x, y) in self.corners {
            if y >= band_top && y <= band_bottom {
                visit(x);
            }
        }
        // Edge crossings against the two horizontal strip lines.
        for i in 0..4 {
            let (x0, y0) = self.corners[i];
            let (x1, y1) = self.corners[(i + 1) % 4];
            for &y_line in &[band_top, band_bottom] {
                let crosses = (y0 - y_line) * (y1 - y_line) <= 0.0 && (y0 - y1).abs() > 1e-6;
                if crosses {
                    let t = (y_line - y0) / (y1 - y0);
                    if (0.0..=1.0).contains(&t) {
                        visit(x0 + t * (x1 - x0));
                    }
                }
            }
        }
        if min_x.is_finite() && max_x.is_finite() && min_x < max_x {
            Some((min_x, max_x))
        } else {
            None
        }
    }
}

/// Compose `translate(dx, dy)` *after* an existing IDML affine.
/// `translate ∘ inner` applied to a point: first inner maps the
/// point, then translate shifts it by (dx, dy). Used by the master-
/// overlay pass to push master-spread coords into the live spread.
/// `None` becomes a pure translation.
/// Stamp a master item: compose its inner `item_transform` (item →
/// master-spread coords) under the page's outer master-overlay
/// transform (`translate(live origin) ∘ MasterPageTransform ∘
/// translate(-master origin)`), yielding the item's transform in
/// live-page space. Generalises the former translation-only stamp so a
/// `MasterPageTransform` carrying rotation/scale is honoured; an
/// identity MPT reduces to the same `(dx, dy)` shift as before.
pub(super) fn compose_outer_matrix(outer: Transform, inner: Option<[f32; 6]>) -> [f32; 6] {
    let inner_t = inner.map(Transform).unwrap_or(Transform::IDENTITY);
    outer.compose(&inner_t).0
}

/// Walk the document's spreads and build per-page wrap-exclusion
/// rectangles in spread coords. Each shape with
/// `TextWrapMode != "None"` contributes its spread-coord bounds
/// inflated by the wrap's offsets. Items without TextWrap, items on
/// no specific page (centroid outside every page bound), and items
/// with active mode `JumpObjectTextWrap` / `NextColumnTextWrap`
/// (which the simple side-shrink heuristic can't model) are skipped.
pub(super) fn collect_wrap_rects_per_page(
    document: &Document,
    spread_page_ranges: &[std::ops::Range<usize>],
    auto_sized_bounds: &HashMap<String, paged_model::Bounds>,
) -> Vec<Vec<WrapShape>> {
    let total_pages: usize = spread_page_ranges.last().map(|r| r.end).unwrap_or(0);
    let mut out: Vec<Vec<WrapShape>> = (0..total_pages).map(|_| Vec::new()).collect();
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let range = spread_page_ranges[spread_idx].clone();
        if range.is_empty() {
            continue;
        }
        // Local page bounds for centroid containment routing.
        let page_bounds: Vec<paged_model::Bounds> = parsed
            .spread
            .pages
            .iter()
            .map(|p| transform_bounds(p.bounds, p.item_transform))
            .collect();
        let route = |aabb: paged_model::Bounds| -> Option<usize> {
            let cx = (aabb.left + aabb.right) * 0.5;
            let cy = (aabb.top + aabb.bottom) * 0.5;
            page_bounds
                .iter()
                .position(|b| cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom)
        };
        let verso: Vec<bool> = page_bounds
            .iter()
            .map(|b| {
                super::metrics::BindingEdge::of_page(b.left, b.width())
                    == super::metrics::BindingEdge::Right
            })
            .collect();
        let mut push = |src: WrapSource<'_>, wrap: Option<paged_model::TextWrap>| {
            let Some(wrap) = wrap else {
                return;
            };
            // Route by the item's own box; the zone may lean past it.
            let routed = WrapShape::from_inner(src.bounds, src.item_transform, [0.0; 4]);
            let Some(local_idx) = route(routed.bounds) else {
                return;
            };
            let Some(shape) = src.shape(wrap, verso[local_idx]) else {
                return;
            };
            let page_idx = range.start + local_idx;
            if page_idx < out.len() {
                out[page_idx].push(shape);
            }
        };
        let styles = &document.styles;
        for f in &parsed.spread.text_frames {
            // W1.7 Phase B: a neighbouring frame wraps around the
            // GROWN box of an auto-sized frame, not its authored
            // undersized rect. Substitute the precomputed grown
            // inner-coord bounds when this frame auto-sizes so the
            // exclusion rect matches the painted box.
            let grown = f
                .self_id
                .as_deref()
                .and_then(|id| auto_sized_bounds.get(id))
                .copied();
            push(
                WrapSource {
                    bounds: grown.unwrap_or(f.bounds),
                    item_transform: f.item_transform,
                    anchors: if grown.is_some() { &[] } else { &f.anchors },
                    subpath_starts: &f.subpath_starts,
                    subpath_open: &f.subpath_open,
                    oval: false,
                    stroke_color: f.stroke_color.as_deref(),
                    stroke_weight: f.stroke_weight,
                },
                styles.item_text_wrap(f.text_wrap, f.applied_object_style.as_deref()),
            );
        }
        for r in &parsed.spread.rectangles {
            push(
                WrapSource {
                    bounds: r.bounds,
                    item_transform: r.item_transform,
                    anchors: &r.anchors,
                    subpath_starts: &r.subpath_starts,
                    subpath_open: &r.subpath_open,
                    oval: false,
                    stroke_color: r.stroke_color.as_deref(),
                    stroke_weight: r.stroke_weight,
                },
                styles.item_text_wrap(r.text_wrap, r.applied_object_style.as_deref()),
            );
        }
        for o in &parsed.spread.ovals {
            push(
                WrapSource {
                    bounds: o.bounds,
                    item_transform: o.item_transform,
                    anchors: &[],
                    subpath_starts: &[],
                    subpath_open: &[],
                    oval: true,
                    stroke_color: o.stroke_color.as_deref(),
                    stroke_weight: o.stroke_weight,
                },
                styles.item_text_wrap(o.text_wrap, o.applied_object_style.as_deref()),
            );
        }
        for p in &parsed.spread.polygons {
            push(
                WrapSource {
                    bounds: p.bounds,
                    item_transform: p.item_transform,
                    anchors: &p.anchors,
                    subpath_starts: &p.subpath_starts,
                    subpath_open: &p.subpath_open,
                    oval: false,
                    stroke_color: p.stroke_color.as_deref(),
                    stroke_weight: p.stroke_weight,
                },
                styles.item_text_wrap(p.text_wrap, p.applied_object_style.as_deref()),
            );
        }
        for l in &parsed.spread.graphic_lines {
            push(
                WrapSource {
                    bounds: l.bounds,
                    item_transform: l.item_transform,
                    anchors: &l.anchors,
                    subpath_starts: &l.subpath_starts,
                    subpath_open: &l.subpath_open,
                    oval: false,
                    stroke_color: l.stroke_color.as_deref(),
                    stroke_weight: l.stroke_weight,
                },
                styles.item_text_wrap(l.text_wrap, l.applied_object_style.as_deref()),
            );
        }
    }
    out
}

/// `CellStyle/$ID/[None]` is IDML's "no style" sentinel. Treat it
/// as absent so the region cascade kicks in.
pub(super) fn is_none_style_id(id: &str) -> bool {
    id == "CellStyle/$ID/[None]" || id == "CellStyle/n" || id.is_empty()
}

/// True for swatch IDs that resolve to "no paint" — used by per-cell
/// stroke override to fall through to the cascaded cell-style colour
/// when the inline `<Cell>` attribute carries `Swatch/None`.
pub(super) fn is_none_swatch_id(id: &str) -> bool {
    // Concept 2 — routed through the shared reserved-swatch
    // classifier; behaviour identical (plus the `Color/None`
    // spelling the canvas-side sites match).
    paged_model::ReservedSwatch::is_none(id)
}

/// True when an `Option<String>` FillColor on a page item should be
/// treated as fully transparent — i.e. no background rect should be
/// emitted at all. Mirrors InDesign's behaviour for both "FillColor
/// attribute absent" and `FillColor="Swatch/None"`. Distinct from the
/// "palette lookup miss" case — when an id is present but unresolved
/// the renderer still falls back to the gray preview swatch.
pub(crate) fn frame_fill_is_transparent(id: Option<&str>) -> bool {
    match id {
        None => true,
        Some(s) => is_none_swatch_id(s),
    }
}

/// True when the frame's stroke would actually paint pixels — i.e.
/// `StrokeColor` resolves to a non-`Swatch/None` paint AND
/// `StrokeWeight > 0`. The drop-shadow module uses this to gate
/// stroke shadows: a stroke shadow without a visible stroke would
/// otherwise leak as a stamped rectangle behind an outline that
/// isn't drawn.
pub(crate) fn frame_stroke_is_visible(stroke_color: Option<&str>, stroke_weight: f32) -> bool {
    if stroke_weight <= 0.0 {
        return false;
    }
    match stroke_color {
        None => false,
        Some(s) => !is_none_swatch_id(s),
    }
}

/// Map an IDML `FontStyle` attribute string to a numeric wght axis
/// value (CSS / fvar convention: 100=Thin, 400=Regular, 700=Bold,
/// 900=Black). Unknown values fall through to 400. Italic / Bold
/// Italic are matched on substring so combined styles still get
/// the right weight; the italic axis is handled separately by
/// loading a different font file (resolver-side).
pub(crate) fn wght_for_font_style(style: Option<&str>) -> f32 {
    let s = match style {
        Some(s) => s,
        None => return 400.0,
    };
    let lower = s.to_ascii_lowercase();
    if lower.contains("thin") || lower.contains("hairline") {
        100.0
    } else if lower.contains("extralight")
        || lower.contains("extra light")
        || lower.contains("ultralight")
    {
        200.0
    } else if lower.contains("light") {
        300.0
    } else if lower.contains("medium") {
        500.0
    } else if lower.contains("semibold")
        || lower.contains("semi bold")
        || lower.contains("demibold")
        || lower.contains("demi bold")
    {
        600.0
    } else if lower.contains("extrabold")
        || lower.contains("extra bold")
        || lower.contains("ultrabold")
    {
        800.0
    } else if lower.contains("bold") {
        700.0
    } else if lower.contains("black") || lower.contains("heavy") {
        900.0
    } else {
        400.0
    }
}

/// Split a paragraph at every `\n` boundary in any run's text into
/// a sequence of sub-paragraphs, each inheriting the parent's
/// style. A `\n` is IDML's paragraph mark `<Br/>`, so each piece is a
/// paragraph of its own: the layout engine sees each one
/// independently, so successive bullet items / address lines / etc.
/// land on their own rows rather than collapsing into
/// glue-separated runs of one paragraph.
///
/// Every piece keeps the range's `SpaceBefore` and `SpaceAfter`, as
/// every paragraph of a range has them: InDesign sets two paragraphs of
/// one range (SpaceBefore 12, SpaceAfter 9, 12 pt Open Sans) 35.4 pt
/// apart, the leading plus both, and an empty paragraph between them
/// takes the same (InDesign 20.0.1,
/// `tools/indesign-export/footnote-paragraphs-probe`, `bodyparas` and
/// `bodyempty`). `tab_list` and other paragraph metadata copy through
/// unchanged.
///
/// A [`FORCED_LINE_BREAK`] splits the same way; the segments record it
/// so the emitter can treat the pieces as lines of ONE paragraph (and
/// count it once in the build's stats) — the emitter withholds the
/// spacing there (`SegmentState::mask`).
#[cfg(test)]
pub(super) fn split_paragraph_at_breaks(
    paragraph: &paged_model::Paragraph,
) -> Vec<paged_model::Paragraph> {
    split_paragraph_into_segments(paragraph)
        .into_iter()
        .map(|s| s.paragraph)
        .collect()
}

/// A forced line break: U+2028, InDesign's Shift+Enter, spelled as the
/// character itself inside IDML `<Content>`. The line ends but the
/// paragraph goes on.
pub(super) const FORCED_LINE_BREAK: char = '\u{2028}';

/// One piece of a paragraph split by [`split_paragraph_into_segments`].
pub(super) struct BreakSegment {
    pub paragraph: paged_model::Paragraph,
    /// Byte offset, in the source paragraph's run text, where this
    /// segment's text begins — so its lines can report paragraph-local
    /// bytes like an unsplit paragraph's.
    pub byte_base: u32,
    /// `false` when a forced line break precedes the segment: it is the
    /// same paragraph's next line, not a paragraph of its own.
    pub opens_paragraph: bool,
    /// `false` when the segment ends at a forced line break.
    pub closes_paragraph: bool,
}

/// The template of a segment that continues a split paragraph: the
/// paragraph's whole-paragraph attributes, without what only its first
/// segment carries (the drop cap, the anchored objects, the
/// footnotes and index markers, which ride the first segment).
fn continuation(paragraph: &paged_model::Paragraph) -> paged_model::Paragraph {
    paged_model::Paragraph {
        paragraph_style: paragraph.paragraph_style.clone(),
        justification: paragraph.justification,
        first_line_indent: paragraph.first_line_indent,
        // W0.2 — whole-paragraph attributes carry to every
        // split sub-paragraph (kinsoku convention).
        left_indent: paragraph.left_indent,
        right_indent: paragraph.right_indent,
        hyphenation: paragraph.hyphenation,
        hyphenation_zone: paragraph.hyphenation_zone,
        hyphenate_after_first: paragraph.hyphenate_after_first,
        hyphenate_before_last: paragraph.hyphenate_before_last,
        hyphenate_words_longer_than: paragraph.hyphenate_words_longer_than,
        hyphenate_capitalized_words: paragraph.hyphenate_capitalized_words,
        hyphenate_last_word: paragraph.hyphenate_last_word,
        hyphenate_across_columns: paragraph.hyphenate_across_columns,
        hyphenate_ladder_limit: paragraph.hyphenate_ladder_limit,
        hyphen_weight: paragraph.hyphen_weight,
        composer: paragraph.composer.clone(),
        keep_lines_together: paragraph.keep_lines_together,
        keep_with_next: paragraph.keep_with_next,
        start_paragraph: paragraph.start_paragraph,
        span_columns: paragraph.span_columns,
        line_setting: paragraph.line_setting,
        grid_alignment: paragraph.grid_alignment,
        grid_align_first_line_only: paragraph.grid_align_first_line_only,
        keep_all_lines_together: paragraph.keep_all_lines_together,
        keep_first_lines: paragraph.keep_first_lines,
        keep_last_lines: paragraph.keep_last_lines,
        rule_above: paragraph.rule_above.clone(),
        rule_below: paragraph.rule_below.clone(),
        // The lists are already layered into the runs (and a segment
        // never recomputes them); they ride along for the read side.
        nested_styles: paragraph.nested_styles.clone(),
        line_styles: paragraph.line_styles.clone(),
        grep_styles: paragraph.grep_styles.clone(),
        space_before: paragraph.space_before,
        space_after: paragraph.space_after,
        tab_list: paragraph.tab_list.clone(),
        bullets_list_type: paragraph.bullets_list_type.clone(),
        bullet_character: paragraph.bullet_character,
        numbering_format: paragraph.numbering_format.clone(),
        bullets_text_after: paragraph.bullets_text_after.clone(),
        numbering_expression: paragraph.numbering_expression.clone(),
        numbering_start_at: paragraph.numbering_start_at,
        numbering_continue: paragraph.numbering_continue,
        list_level: paragraph.list_level,
        bullets_character_style: paragraph.bullets_character_style.clone(),
        bullets_and_numbering_digits_character_style: paragraph
            .bullets_and_numbering_digits_character_style
            .clone(),
        applied_numbering_list: paragraph.applied_numbering_list.clone(),
        // Drop cap + anchored frames are first-paragraph-only;
        // sub-paragraphs after a `\n` reset to defaults.
        drop_cap_characters: 0,
        drop_cap_lines: 0,
        drop_cap_detail: 0,
        // Spacing set on the range carries to every segment.
        minimum_word_spacing: paragraph.minimum_word_spacing,
        desired_word_spacing: paragraph.desired_word_spacing,
        maximum_word_spacing: paragraph.maximum_word_spacing,
        minimum_letter_spacing: paragraph.minimum_letter_spacing,
        desired_letter_spacing: paragraph.desired_letter_spacing,
        maximum_letter_spacing: paragraph.maximum_letter_spacing,
        minimum_glyph_scaling: paragraph.minimum_glyph_scaling,
        desired_glyph_scaling: paragraph.desired_glyph_scaling,
        maximum_glyph_scaling: paragraph.maximum_glyph_scaling,
        overprint_fill: paragraph.overprint_fill,
        overprint_stroke: paragraph.overprint_stroke,
        // Kinsoku / Mojikumi apply to the whole paragraph.
        kinsoku_set: paragraph.kinsoku_set.clone(),
        kinsoku_type: paragraph.kinsoku_type.clone(),
        mojikumi_table: paragraph.mojikumi_table.clone(),
        mojikumi_set: paragraph.mojikumi_set.clone(),
        anchored_frames: Vec::new(),
        anchored_frame_offsets: Vec::new(),
        runs: Vec::new(),
        table: None,
        table_anchor: None,
        // Sub-paragraphs after a `\n` reset markers too
        // (matches anchored-frame convention above).
        footnotes: Vec::new(),
        index_markers: Vec::new(),
        endnote_references: Vec::new(),
        endnote_ranges: Vec::new(),
        notes: Vec::new(),
    }
}

/// [`split_paragraph_at_breaks`], keeping where each piece came from and
/// whether a `\n` (a paragraph boundary) or a [`FORCED_LINE_BREAK`] (a
/// line boundary inside one paragraph) delimits it.
pub(super) fn split_paragraph_into_segments(
    paragraph: &paged_model::Paragraph,
) -> Vec<BreakSegment> {
    // Walk runs in order; for each run, split text at each break and
    // emit the leading segment into the in-progress sub-paragraph,
    // then close the sub-paragraph and start a new one.
    let mut subs: Vec<BreakSegment> = Vec::new();
    let mut current = paged_model::Paragraph {
        paragraph_style: paragraph.paragraph_style.clone(),
        justification: paragraph.justification,
        first_line_indent: paragraph.first_line_indent,
        // W0.2 — left/right indent and the rule structs are
        // whole-paragraph attributes; every split sub-paragraph
        // inherits them (same convention as kinsoku / indents below).
        left_indent: paragraph.left_indent,
        right_indent: paragraph.right_indent,
        hyphenation: paragraph.hyphenation,
        hyphenation_zone: paragraph.hyphenation_zone,
        hyphenate_after_first: paragraph.hyphenate_after_first,
        hyphenate_before_last: paragraph.hyphenate_before_last,
        hyphenate_words_longer_than: paragraph.hyphenate_words_longer_than,
        hyphenate_capitalized_words: paragraph.hyphenate_capitalized_words,
        hyphenate_last_word: paragraph.hyphenate_last_word,
        hyphenate_across_columns: paragraph.hyphenate_across_columns,
        hyphenate_ladder_limit: paragraph.hyphenate_ladder_limit,
        hyphen_weight: paragraph.hyphen_weight,
        composer: paragraph.composer.clone(),
        keep_lines_together: paragraph.keep_lines_together,
        keep_with_next: paragraph.keep_with_next,
        start_paragraph: paragraph.start_paragraph,
        span_columns: paragraph.span_columns,
        line_setting: paragraph.line_setting,
        grid_alignment: paragraph.grid_alignment,
        grid_align_first_line_only: paragraph.grid_align_first_line_only,
        keep_all_lines_together: paragraph.keep_all_lines_together,
        keep_first_lines: paragraph.keep_first_lines,
        keep_last_lines: paragraph.keep_last_lines,
        rule_above: paragraph.rule_above.clone(),
        rule_below: paragraph.rule_below.clone(),
        // The lists are already layered into the runs (and a segment
        // never recomputes them); they ride along for the read side.
        nested_styles: paragraph.nested_styles.clone(),
        line_styles: paragraph.line_styles.clone(),
        grep_styles: paragraph.grep_styles.clone(),
        space_before: paragraph.space_before,
        space_after: paragraph.space_after,
        tab_list: paragraph.tab_list.clone(),
        bullets_list_type: paragraph.bullets_list_type.clone(),
        bullet_character: paragraph.bullet_character,
        numbering_format: paragraph.numbering_format.clone(),
        bullets_text_after: paragraph.bullets_text_after.clone(),
        numbering_expression: paragraph.numbering_expression.clone(),
        numbering_start_at: paragraph.numbering_start_at,
        numbering_continue: paragraph.numbering_continue,
        list_level: paragraph.list_level,
        bullets_character_style: paragraph.bullets_character_style.clone(),
        bullets_and_numbering_digits_character_style: paragraph
            .bullets_and_numbering_digits_character_style
            .clone(),
        applied_numbering_list: paragraph.applied_numbering_list.clone(),
        // Drop-cap + anchored frames carry on the FIRST sub-paragraph
        // only; the splits below clone from the source paragraph and
        // overwrite these to defaults so the cap doesn't repeat.
        drop_cap_characters: paragraph.drop_cap_characters,
        drop_cap_lines: paragraph.drop_cap_lines,
        drop_cap_detail: paragraph.drop_cap_detail,
        // Spacing set on the range carries to every segment.
        minimum_word_spacing: paragraph.minimum_word_spacing,
        desired_word_spacing: paragraph.desired_word_spacing,
        maximum_word_spacing: paragraph.maximum_word_spacing,
        minimum_letter_spacing: paragraph.minimum_letter_spacing,
        desired_letter_spacing: paragraph.desired_letter_spacing,
        maximum_letter_spacing: paragraph.maximum_letter_spacing,
        minimum_glyph_scaling: paragraph.minimum_glyph_scaling,
        desired_glyph_scaling: paragraph.desired_glyph_scaling,
        maximum_glyph_scaling: paragraph.maximum_glyph_scaling,
        overprint_fill: paragraph.overprint_fill,
        overprint_stroke: paragraph.overprint_stroke,
        // Kinsoku / Mojikumi apply to the whole paragraph; every
        // split sub-paragraph inherits the same set.
        kinsoku_set: paragraph.kinsoku_set.clone(),
        kinsoku_type: paragraph.kinsoku_type.clone(),
        mojikumi_table: paragraph.mojikumi_table.clone(),
        mojikumi_set: paragraph.mojikumi_set.clone(),
        anchored_frames: paragraph.anchored_frames.clone(),
        anchored_frame_offsets: paragraph.anchored_frame_offsets.clone(),
        runs: Vec::new(),
        table: None,
        table_anchor: None,
        // Phase 5 — footnotes / index markers ride the FIRST
        // sub-paragraph only (matches the anchored-frame +
        // drop-cap convention above); subsequent splits start with
        // empty vecs so the markers don't duplicate.
        footnotes: paragraph.footnotes.clone(),
        index_markers: paragraph.index_markers.clone(),
        endnote_references: Vec::new(),
        endnote_ranges: Vec::new(),
        notes: Vec::new(),
    };
    let is_break = |c: char| c == '\n' || c == FORCED_LINE_BREAK;
    // The table stands at a character of the paragraph, on a line of its
    // own (asked of InDesign 20.0.1, `table-inline-text`): its place is a
    // cut that ends the line before it — not the paragraph — and the
    // table is a segment of its own between the two lines.
    let mut table_byte: Option<usize> = paragraph.table.as_ref().map(|_| {
        let total: u32 = paragraph
            .runs
            .iter()
            .map(|r| r.text.chars().count() as u32)
            .sum();
        paragraph.byte_of_char_offset(paragraph.table_anchor.unwrap_or(total).min(total))
    });
    // Set from the table's segment until something follows it: a break
    // right after the table ends the TABLE's line (a paragraph mark also
    // ends its paragraph), so it opens no empty line of its own.
    let mut after_table = false;
    // Byte offset of the current run within the paragraph's text, and
    // where/how the in-progress segment began.
    let mut pos: usize = 0;
    let mut current_opens = true;
    let mut current_base: u32 = 0;
    for run in &paragraph.runs {
        let table_here = table_byte
            .filter(|&t| t >= pos && t < pos + run.text.len())
            .map(|t| t - pos);
        if table_here.is_none() && !run.text.contains(is_break) {
            if !run.text.is_empty() {
                after_table = false;
            }
            current.runs.push(run.clone());
            pos += run.text.len();
            continue;
        }
        // The cuts in this run, in text order; the table goes before a
        // break at the same byte (it stands before that character).
        let mut cuts: Vec<(usize, Option<char>)> = run
            .text
            .char_indices()
            .filter(|(_, c)| is_break(*c))
            .map(|(i, c)| (i, Some(c)))
            .collect();
        if let Some(k) = table_here {
            let at = cuts.partition_point(|&(i, _)| i < k);
            cuts.insert(at, (k, None));
            table_byte = None;
        }
        let mut seg_start = 0;
        for (i, cut) in cuts {
            let seg = &run.text[seg_start..i];
            if !seg.is_empty() {
                let mut copy = run.clone();
                copy.text = seg.to_string();
                current.runs.push(copy);
                after_table = false;
            }
            let Some(ch) = cut else {
                push_table_segment(
                    paragraph,
                    &mut subs,
                    &mut current,
                    &mut current_opens,
                    (current_base, (pos + i) as u32),
                );
                after_table = true;
                seg_start = i;
                current_base = (pos + i) as u32;
                continue;
            };
            if after_table && current.runs.is_empty() {
                // The break right after the table ends the table's line.
                if let Some(table_seg) = subs.last_mut() {
                    table_seg.closes_paragraph = ch == '\n';
                }
                after_table = false;
                current_opens = ch == '\n';
                seg_start = i + ch.len_utf8();
                current_base = (pos + seg_start) as u32;
                continue;
            }
            after_table = false;
            // If the about-to-be-closed sub-paragraph has no runs
            // (the previous segment ended with a break and produced
            // a terminator straight away), surface the run's
            // character attributes via a zero-text run so the
            // empty-paragraph emit branch can read its PointSize.
            // Without this, an empty paragraph inside a 24pt
            // `<Br/><Br/>` falls through to the paragraph style's
            // PointSize (or the default 12pt), collapsing the leading
            // from 28.8pt to 14.4pt.
            if current.runs.is_empty() {
                let mut hint = run.clone();
                hint.text = String::new();
                current.runs.push(hint);
            }
            // Close the current sub-paragraph and start a new one.
            let mut next = continuation(paragraph);
            std::mem::swap(&mut current, &mut next);
            // Keep empty sub-paragraphs — `<Br/><Br/>` and similar
            // patterns mean "advance one line of vertical space".
            // The emitter renders them as a single line-height step
            // (no glyphs) so the surrounding text keeps its rhythm.
            subs.push(BreakSegment {
                paragraph: next,
                byte_base: current_base,
                opens_paragraph: current_opens,
                closes_paragraph: ch == '\n',
            });
            current_opens = ch == '\n';
            seg_start = i + ch.len_utf8();
            current_base = (pos + seg_start) as u32;
        }
        let tail = &run.text[seg_start..];
        if !tail.is_empty() {
            let mut copy = run.clone();
            copy.text = tail.to_string();
            current.runs.push(copy);
            after_table = false;
        }
        pos += run.text.len();
    }
    // A table after all of the text (or with no anchor) ends the
    // paragraph: the model's "runs, then table" paragraph.
    if table_byte.is_some() {
        push_table_segment(
            paragraph,
            &mut subs,
            &mut current,
            &mut current_opens,
            (current_base, pos as u32),
        );
        after_table = true;
        current_base = pos as u32;
    }
    if after_table && current.runs.iter().all(|r| r.text.is_empty()) {
        current.runs.clear();
        if let Some(table_seg) = subs.last_mut() {
            table_seg.closes_paragraph = true;
        }
    }
    // A forced line break that ends the paragraph still leaves the empty
    // line after it (the paragraph mark stands on it), so that segment
    // stays even with no runs.
    if current.runs.is_empty() && !current_opens && !after_table {
        if let Some(run) = paragraph.runs.last() {
            let mut hint = run.clone();
            hint.text = String::new();
            current.runs.push(hint);
        }
    }
    // Flush the trailing sub-paragraph.
    if !current.runs.is_empty() {
        subs.push(BreakSegment {
            paragraph: current,
            byte_base: current_base,
            opens_paragraph: current_opens,
            closes_paragraph: true,
        });
    }
    // P-25 guard: drop a trailing sub-paragraph whose every run is
    // empty or `\n`-only. The split loop above already discards the
    // `current` working sub when its runs vec is empty, but a
    // pathological run carrying ONLY `\n` characters in its text
    // would seed a sub with a zero-text hint run that has no visible
    // glyphs yet still triggers bullet-marker emission for
    // NumberedList paragraphs. Drop those at the tail so the
    // numbering counter doesn't double-fire on the visible line.
    // Stops short of dropping interior empty sub-paragraphs because
    // consecutive `<Br/>` pairs intentionally render as empty
    // vertical-leading slots — and never drops the line a trailing
    // forced line break leaves.
    while subs.len() > 1
        && subs
            .last()
            .map(|s| {
                s.opens_paragraph
                    && s.paragraph.table.is_none()
                    && s.paragraph
                        .runs
                        .iter()
                        .all(|r| r.text.is_empty() || r.text.chars().all(|c| c == '\n'))
            })
            .unwrap_or(false)
    {
        subs.pop();
        if let Some(last) = subs.last_mut() {
            last.closes_paragraph = true;
        }
    }
    if subs.is_empty() {
        // Defensive: the original was all `\n`s. Return a single
        // empty paragraph to keep the upstream loop's stat
        // bookkeeping consistent without rendering anything.
        subs.push(BreakSegment {
            byte_base: 0,
            opens_paragraph: true,
            closes_paragraph: true,
            paragraph: paged_model::Paragraph {
                paragraph_style: paragraph.paragraph_style.clone(),
                justification: paragraph.justification,
                first_line_indent: paragraph.first_line_indent,
                // W0.2 — whole-paragraph attributes (carry from source).
                left_indent: paragraph.left_indent,
                right_indent: paragraph.right_indent,
                hyphenation: paragraph.hyphenation,
                hyphenation_zone: paragraph.hyphenation_zone,
                hyphenate_after_first: paragraph.hyphenate_after_first,
                hyphenate_before_last: paragraph.hyphenate_before_last,
                hyphenate_words_longer_than: paragraph.hyphenate_words_longer_than,
                hyphenate_capitalized_words: paragraph.hyphenate_capitalized_words,
                hyphenate_last_word: paragraph.hyphenate_last_word,
                hyphenate_across_columns: paragraph.hyphenate_across_columns,
                hyphenate_ladder_limit: paragraph.hyphenate_ladder_limit,
                hyphen_weight: paragraph.hyphen_weight,
                composer: paragraph.composer.clone(),
                keep_lines_together: paragraph.keep_lines_together,
                keep_with_next: paragraph.keep_with_next,
                start_paragraph: paragraph.start_paragraph,
                span_columns: paragraph.span_columns,
                line_setting: paragraph.line_setting,
                grid_alignment: paragraph.grid_alignment,
                grid_align_first_line_only: paragraph.grid_align_first_line_only,
                keep_all_lines_together: paragraph.keep_all_lines_together,
                keep_first_lines: paragraph.keep_first_lines,
                keep_last_lines: paragraph.keep_last_lines,
                rule_above: paragraph.rule_above.clone(),
                rule_below: paragraph.rule_below.clone(),
                // The lists are already layered into the runs (and a segment
                // never recomputes them); they ride along for the read side.
                nested_styles: paragraph.nested_styles.clone(),
                line_styles: paragraph.line_styles.clone(),
                grep_styles: paragraph.grep_styles.clone(),
                space_before: paragraph.space_before,
                space_after: paragraph.space_after,
                tab_list: paragraph.tab_list.clone(),
                bullets_list_type: paragraph.bullets_list_type.clone(),
                bullet_character: paragraph.bullet_character,
                numbering_format: paragraph.numbering_format.clone(),
                bullets_text_after: paragraph.bullets_text_after.clone(),
                numbering_expression: paragraph.numbering_expression.clone(),
                numbering_start_at: paragraph.numbering_start_at,
                numbering_continue: paragraph.numbering_continue,
                list_level: paragraph.list_level,
                bullets_character_style: paragraph.bullets_character_style.clone(),
                bullets_and_numbering_digits_character_style: paragraph
                    .bullets_and_numbering_digits_character_style
                    .clone(),
                applied_numbering_list: paragraph.applied_numbering_list.clone(),
                // All-`\n` source paragraph: defensive placeholder.
                // Drop cap + anchored frames don't apply to a glyph-less
                // paragraph; default them.
                drop_cap_characters: 0,
                drop_cap_lines: 0,
                drop_cap_detail: 0,
                // Spacing set on the range carries to every segment.
                minimum_word_spacing: paragraph.minimum_word_spacing,
                desired_word_spacing: paragraph.desired_word_spacing,
                maximum_word_spacing: paragraph.maximum_word_spacing,
                minimum_letter_spacing: paragraph.minimum_letter_spacing,
                desired_letter_spacing: paragraph.desired_letter_spacing,
                maximum_letter_spacing: paragraph.maximum_letter_spacing,
                minimum_glyph_scaling: paragraph.minimum_glyph_scaling,
                desired_glyph_scaling: paragraph.desired_glyph_scaling,
                maximum_glyph_scaling: paragraph.maximum_glyph_scaling,
                overprint_fill: paragraph.overprint_fill,
                overprint_stroke: paragraph.overprint_stroke,
                kinsoku_set: paragraph.kinsoku_set.clone(),
                kinsoku_type: paragraph.kinsoku_type.clone(),
                mojikumi_table: paragraph.mojikumi_table.clone(),
                mojikumi_set: paragraph.mojikumi_set.clone(),
                anchored_frames: Vec::new(),
                anchored_frame_offsets: Vec::new(),
                runs: Vec::new(),
                table: None,
                table_anchor: None,
                footnotes: Vec::new(),
                index_markers: Vec::new(),
                endnote_references: Vec::new(),
                endnote_ranges: Vec::new(),
                notes: Vec::new(),
            },
        });
    }
    // ADR 170 — each segment takes the endnote entries its own markers
    // stand for, in order (the k-th marker of a segment is its k-th).
    if !paragraph.endnote_references.is_empty() || !paragraph.endnote_ranges.is_empty() {
        let mut refs = paragraph.endnote_references.iter().cloned();
        let mut ranges = paragraph.endnote_ranges.iter().cloned();
        for seg in subs.iter_mut() {
            let count = |marker: char| {
                seg.paragraph
                    .runs
                    .iter()
                    .map(|r| r.text.matches(marker).count())
                    .sum::<usize>()
            };
            let (n_ref, n_num) = (
                count(paged_model::ENDNOTE_REFERENCE_MARKER),
                count(paged_model::ENDNOTE_NUMBER_MARKER),
            );
            seg.paragraph.endnote_references = refs.by_ref().take(n_ref).collect();
            seg.paragraph.endnote_ranges = ranges.by_ref().take(n_num).collect();
        }
    }
    subs
}

/// Close the line before a table (when it holds text: it ends the line,
/// not the paragraph) and push the table's own segment at `byte_base`.
/// The text after the table continues the paragraph on a new line; when
/// the table opens the paragraph, the text after it no longer does.
///
/// A table with no anchor keeps the reading the engine had before the
/// anchor existed: its text is a paragraph of its own and the table
/// follows it.
fn push_table_segment(
    paragraph: &paged_model::Paragraph,
    subs: &mut Vec<BreakSegment>,
    current: &mut paged_model::Paragraph,
    current_opens: &mut bool,
    (current_base, byte_base): (u32, u32),
) {
    let anchored = paragraph.table_anchor.is_some();
    if current.runs.iter().any(|r| !r.text.is_empty()) {
        let text = std::mem::replace(current, continuation(paragraph));
        subs.push(BreakSegment {
            byte_base: current_base,
            paragraph: text,
            opens_paragraph: *current_opens,
            closes_paragraph: !anchored,
        });
        *current_opens = !anchored;
    } else {
        // Nothing before the table on its line: the table's line is the
        // paragraph's first when the paragraph opens here.
        current.runs.clear();
        current.space_before = None;
        current.drop_cap_characters = 0;
        current.drop_cap_lines = 0;
        current.drop_cap_detail = 0;
    }
    subs.push(BreakSegment {
        paragraph: paged_model::Paragraph {
            table: paragraph.table.clone(),
            ..continuation(paragraph)
        },
        byte_base,
        opens_paragraph: *current_opens,
        closes_paragraph: false,
    });
    *current_opens = false;
}
