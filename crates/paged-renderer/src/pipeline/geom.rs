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

use paged_compose::{
    DisplayList, GlyphCacheKey,
    GlyphOutliner, Transform,
};
use paged_parse::PathAnchor;
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
    pub(super) bounds_in_spread: paged_parse::Bounds,
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

pub(crate) fn transform_bounds(b: paged_parse::Bounds, m: Option<[f32; 6]>) -> paged_parse::Bounds {
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
    paged_parse::Bounds {
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
#[derive(Debug, Clone, Copy)]
pub(super) struct WrapShape {
    pub(super) bounds: paged_parse::Bounds,
    pub(super) corners: [(f32, f32); 4],
}

impl WrapShape {
    /// Build from an inner-coord `Bounds`, an optional ItemTransform,
    /// and per-side wrap offsets `[top, left, bottom, right]`. The
    /// offsets inflate the unrotated source rect *before* the
    /// transform applies so the polygon stays aligned with the host's
    /// rotation (offset is in inner-coord points, same as InDesign).
    pub(super) fn from_inner(b: paged_parse::Bounds, m: Option<[f32; 6]>, offsets: [f32; 4]) -> Self {
        let inner = paged_parse::Bounds {
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
        let bounds = paged_parse::Bounds {
            top: min_y,
            left: min_x,
            bottom: max_y,
            right: max_x,
        };
        Self { bounds, corners }
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
    auto_sized_bounds: &HashMap<String, paged_parse::Bounds>,
) -> Vec<Vec<WrapShape>> {
    let total_pages: usize = spread_page_ranges.last().map(|r| r.end).unwrap_or(0);
    let mut out: Vec<Vec<WrapShape>> = (0..total_pages).map(|_| Vec::new()).collect();
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let range = spread_page_ranges[spread_idx].clone();
        if range.is_empty() {
            continue;
        }
        // Local page bounds for centroid containment routing.
        let page_bounds: Vec<paged_parse::Bounds> = parsed
            .spread
            .pages
            .iter()
            .map(|p| transform_bounds(p.bounds, p.item_transform))
            .collect();
        let route = |aabb: paged_parse::Bounds| -> Option<usize> {
            let cx = (aabb.left + aabb.right) * 0.5;
            let cy = (aabb.top + aabb.bottom) * 0.5;
            page_bounds
                .iter()
                .position(|b| cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom)
        };
        let push = |out: &mut Vec<Vec<WrapShape>>,
                    inner_bounds: paged_parse::Bounds,
                    item_transform: Option<[f32; 6]>,
                    wrap: paged_parse::TextWrap| {
            if !wrap.mode.is_active() {
                return;
            }
            // Treat BoundingBoxTextWrap and ContourTextWrap as
            // bounding-box exclusions. ContourTextWrap with
            // `ContourType=BoundingBox` (the default that InDesign
            // emits for plain rectangle hosts) is identical; richer
            // contour types degrade to their AABB which is still a
            // useful first-cut. JumpObject / NextColumn keep being
            // skipped — they need column-level layout we don't yet
            // model, and approximating them as side-shrink makes
            // matters worse.
            if !matches!(
                wrap.mode,
                paged_parse::TextWrapMode::BoundingBoxTextWrap
                    | paged_parse::TextWrapMode::ContourTextWrap
            ) {
                return;
            }
            let shape = WrapShape::from_inner(inner_bounds, item_transform, wrap.offsets);
            if let Some(local_idx) = route(shape.bounds) {
                let page_idx = range.start + local_idx;
                if page_idx < out.len() {
                    out[page_idx].push(shape);
                }
            }
        };
        for f in &parsed.spread.text_frames {
            if let Some(w) = f.text_wrap {
                // W1.7 Phase B: a neighbouring frame wraps around the
                // GROWN box of an auto-sized frame, not its authored
                // undersized rect. Substitute the precomputed grown
                // inner-coord bounds when this frame auto-sizes so the
                // exclusion rect matches the painted box.
                let wrap_bounds = f
                    .self_id
                    .as_deref()
                    .and_then(|id| auto_sized_bounds.get(id))
                    .copied()
                    .unwrap_or(f.bounds);
                push(&mut out, wrap_bounds, f.item_transform, w);
            }
        }
        for r in &parsed.spread.rectangles {
            if let Some(w) = r.text_wrap {
                push(&mut out, r.bounds, r.item_transform, w);
            }
        }
        for o in &parsed.spread.ovals {
            if let Some(w) = o.text_wrap {
                push(&mut out, o.bounds, o.item_transform, w);
            }
        }
        for p in &parsed.spread.polygons {
            if let Some(w) = p.text_wrap {
                push(&mut out, p.bounds, p.item_transform, w);
            }
        }
        for l in &parsed.spread.graphic_lines {
            if let Some(w) = l.text_wrap {
                push(&mut out, l.bounds, l.item_transform, w);
            }
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
    paged_parse::graphic::ReservedSwatch::is_none(id)
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
pub(super) fn wght_for_font_style(style: Option<&str>) -> f32 {
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
/// style. Used to honour IDML `<Br/>` (which serialises as `\n`)
/// as a forced line break: the layout engine sees each sub-
/// paragraph independently, so successive bullet items / address
/// lines / etc. land on their own rows rather than collapsing
/// into glue-separated runs of one paragraph.
///
/// `SpaceBefore` is suppressed on every sub-paragraph past the
/// first so consecutive lines in the same logical paragraph don't
/// accumulate extra leading. `tab_list` and other paragraph
/// metadata copy through unchanged.
pub(super) fn split_paragraph_at_breaks(paragraph: &paged_parse::Paragraph) -> Vec<paged_parse::Paragraph> {
    // Walk runs in order; for each run, split text at '\n' and
    // emit the leading segment into the in-progress sub-paragraph,
    // then close the sub-paragraph and start a new one.
    let mut subs: Vec<paged_parse::Paragraph> = Vec::new();
    let mut current = paged_parse::Paragraph {
        paragraph_style: paragraph.paragraph_style.clone(),
        justification: paragraph.justification,
        first_line_indent: paragraph.first_line_indent,
        // W0.2 — left/right indent and the rule structs are
        // whole-paragraph attributes; every split sub-paragraph
        // inherits them (same convention as kinsoku / indents below).
        left_indent: paragraph.left_indent,
        right_indent: paragraph.right_indent,
        hyphenation: paragraph.hyphenation,
        keep_lines_together: paragraph.keep_lines_together,
        keep_with_next: paragraph.keep_with_next,
        rule_above: paragraph.rule_above.clone(),
        rule_below: paragraph.rule_below.clone(),
        space_before: paragraph.space_before,
        space_after: None, // applied to last sub-paragraph only
        tab_list: paragraph.tab_list.clone(),
        bullets_list_type: paragraph.bullets_list_type.clone(),
        bullet_character: paragraph.bullet_character,
        numbering_format: paragraph.numbering_format.clone(),
        applied_numbering_list: paragraph.applied_numbering_list.clone(),
        // Drop-cap + anchored frames carry on the FIRST sub-paragraph
        // only; the splits below clone from the source paragraph and
        // overwrite these to defaults so the cap doesn't repeat.
        drop_cap_characters: paragraph.drop_cap_characters,
        drop_cap_lines: paragraph.drop_cap_lines,
        drop_cap_detail: paragraph.drop_cap_detail,
        overprint_fill: paragraph.overprint_fill,
        overprint_stroke: paragraph.overprint_stroke,
        // Kinsoku / Mojikumi apply to the whole paragraph; every
        // split sub-paragraph inherits the same set.
        kinsoku_set: paragraph.kinsoku_set.clone(),
        kinsoku_type: paragraph.kinsoku_type.clone(),
        mojikumi_table: paragraph.mojikumi_table.clone(),
        mojikumi_set: paragraph.mojikumi_set.clone(),
        anchored_frames: paragraph.anchored_frames.clone(),
        runs: Vec::new(),
        table: None,
        // Phase 5 — footnotes / index markers ride the FIRST
        // sub-paragraph only (matches the anchored-frame +
        // drop-cap convention above); subsequent splits start with
        // empty vecs so the markers don't duplicate.
        footnotes: paragraph.footnotes.clone(),
        index_markers: paragraph.index_markers.clone(),
    };
    for run in &paragraph.runs {
        if !run.text.contains('\n') {
            current.runs.push(run.clone());
            continue;
        }
        let segments: Vec<&str> = run.text.split('\n').collect();
        for (i, seg) in segments.iter().enumerate() {
            if !seg.is_empty() {
                let mut copy = run.clone();
                copy.text = (*seg).to_string();
                current.runs.push(copy);
            }
            if i + 1 < segments.len() {
                // If the about-to-be-closed sub-paragraph has no runs
                // (the previous segment ended with a `\n` and produced
                // a paragraph terminator straight away), surface the
                // run's character attributes via a zero-text run so
                // the empty-paragraph emit branch can read its
                // PointSize. Without this, an empty paragraph inside
                // a 24pt `<Br/><Br/>` falls through to the paragraph
                // style's PointSize (or the default 12pt), collapsing
                // the leading from 28.8pt to 14.4pt.
                if current.runs.is_empty() {
                    let mut hint = run.clone();
                    hint.text = String::new();
                    current.runs.push(hint);
                }
                // Close the current sub-paragraph and start a new
                // one. Discard empty sub-paragraphs (consecutive
                // `\n`s, common at the end of bullet lists).
                let mut next = paged_parse::Paragraph {
                    paragraph_style: paragraph.paragraph_style.clone(),
                    justification: paragraph.justification,
                    first_line_indent: paragraph.first_line_indent,
                    // W0.2 — whole-paragraph attributes carry to every
                    // split sub-paragraph (kinsoku convention).
                    left_indent: paragraph.left_indent,
                    right_indent: paragraph.right_indent,
                    hyphenation: paragraph.hyphenation,
                    keep_lines_together: paragraph.keep_lines_together,
                    keep_with_next: paragraph.keep_with_next,
                    rule_above: paragraph.rule_above.clone(),
                    rule_below: paragraph.rule_below.clone(),
                    space_before: None,
                    space_after: None,
                    tab_list: paragraph.tab_list.clone(),
                    bullets_list_type: paragraph.bullets_list_type.clone(),
                    bullet_character: paragraph.bullet_character,
                    numbering_format: paragraph.numbering_format.clone(),
                    applied_numbering_list: paragraph.applied_numbering_list.clone(),
                    // Drop cap + anchored frames are first-paragraph-only;
                    // sub-paragraphs after a `\n` reset to defaults.
                    drop_cap_characters: 0,
                    drop_cap_lines: 0,
                    drop_cap_detail: 0,
                    overprint_fill: paragraph.overprint_fill,
                    overprint_stroke: paragraph.overprint_stroke,
                    // Kinsoku / Mojikumi apply to the whole paragraph.
                    kinsoku_set: paragraph.kinsoku_set.clone(),
                    kinsoku_type: paragraph.kinsoku_type.clone(),
                    mojikumi_table: paragraph.mojikumi_table.clone(),
                    mojikumi_set: paragraph.mojikumi_set.clone(),
                    anchored_frames: Vec::new(),
                    runs: Vec::new(),
                    table: None,
                    // Sub-paragraphs after a `\n` reset markers too
                    // (matches anchored-frame convention above).
                    footnotes: Vec::new(),
                    index_markers: Vec::new(),
                };
                std::mem::swap(&mut current, &mut next);
                // Keep empty sub-paragraphs — `<Br/><Br/>` and similar
                // patterns mean "advance one line of vertical space".
                // The emitter renders them as a single line-height
                // step (no glyphs) so the surrounding text keeps its
                // visual rhythm.
                subs.push(next);
            }
        }
    }
    // Flush the trailing sub-paragraph + propagate the original
    // SpaceAfter so the chain's vertical spacing matches.
    if !current.runs.is_empty() {
        current.space_after = paragraph.space_after;
        subs.push(current);
    } else if let Some(last) = subs.last_mut() {
        last.space_after = paragraph.space_after;
    }
    // P-25 guard: drop a trailing sub-paragraph whose every run is
    // empty or `\n`-only. The split loop above already discards the
    // `current` working sub when its runs vec is empty, but a
    // pathological run carrying ONLY `\n` characters in its text
    // would seed a sub with a zero-text hint run (set at line ~5891)
    // that has no visible glyphs yet still triggers bullet-marker
    // emission for NumberedList paragraphs. Drop those at the tail
    // so the numbering counter doesn't double-fire on the visible
    // line. Stops short of dropping interior empty sub-paragraphs
    // because consecutive `<Br/>` pairs intentionally render as
    // empty vertical-leading slots.
    while subs.len() > 1
        && subs
            .last()
            .map(|p| {
                p.runs
                    .iter()
                    .all(|r| r.text.is_empty() || r.text.chars().all(|c| c == '\n'))
            })
            .unwrap_or(false)
    {
        // Carry the dropped tail's space_after over to the new last.
        let dropped = subs.pop().expect("len > 1 just checked");
        if let Some(last) = subs.last_mut() {
            last.space_after = last.space_after.or(dropped.space_after);
        }
    }
    if subs.is_empty() {
        // Defensive: the original was all `\n`s. Return a single
        // empty paragraph to keep the upstream loop's stat
        // bookkeeping consistent without rendering anything.
        subs.push(paged_parse::Paragraph {
            paragraph_style: paragraph.paragraph_style.clone(),
            justification: paragraph.justification,
            first_line_indent: paragraph.first_line_indent,
            // W0.2 — whole-paragraph attributes (carry from source).
            left_indent: paragraph.left_indent,
            right_indent: paragraph.right_indent,
            hyphenation: paragraph.hyphenation,
            keep_lines_together: paragraph.keep_lines_together,
            keep_with_next: paragraph.keep_with_next,
            rule_above: paragraph.rule_above.clone(),
            rule_below: paragraph.rule_below.clone(),
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            tab_list: paragraph.tab_list.clone(),
            bullets_list_type: paragraph.bullets_list_type.clone(),
            bullet_character: paragraph.bullet_character,
            numbering_format: paragraph.numbering_format.clone(),
            applied_numbering_list: paragraph.applied_numbering_list.clone(),
            // All-`\n` source paragraph: defensive placeholder.
            // Drop cap + anchored frames don't apply to a glyph-less
            // paragraph; default them.
            drop_cap_characters: 0,
            drop_cap_lines: 0,
            drop_cap_detail: 0,
            overprint_fill: paragraph.overprint_fill,
            overprint_stroke: paragraph.overprint_stroke,
            kinsoku_set: paragraph.kinsoku_set.clone(),
            kinsoku_type: paragraph.kinsoku_type.clone(),
            mojikumi_table: paragraph.mojikumi_table.clone(),
            mojikumi_set: paragraph.mojikumi_set.clone(),
            anchored_frames: Vec::new(),
            runs: Vec::new(),
            table: None,
            footnotes: Vec::new(),
            index_markers: Vec::new(),
        });
    }
    subs
}
