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

//! Anchored (inline / above-line / custom-positioned) frame emission:
//! per-paragraph anchored TextFrames recurse into their stories,
//! anchored Rectangles emit fills and deferred images via
//! [`AnchoredImageEmit`], bounded by `MAX_ANCHORED_STORY_RECURSION`.

use super::*;

/// One image-bearing anchored Rectangle captured during the body /
/// master story pass. The post-pass in `build_document` drains
/// these and routes each through `emit_rectangle_image` with the
/// per-page + decoded caches already in scope. Made `pub` so the
/// Perf-BodyStory cache can hold these entries between rebuilds —
/// see `BodyStoryEmissionDelta`.
#[derive(Debug, Clone)]
pub struct AnchoredImageEmit {
    pub target_page: usize,
    pub place_x: f32,
    pub place_y: f32,
    pub width: f32,
    pub height: f32,
    /// Cloned so the post-pass doesn't borrow the source
    /// `AnchoredFrame` (which lives inside the parsed Story tree). We
    /// only need image_link / image_item_transform / self_id for the
    /// rectangle synthesis below, so the clone is cheap.
    pub af: paged_parse::AnchoredFrame,
}

/// Hard cap on `anchored_recursion_depth`. Real-world IDMLs nest at
/// most 1–2 deep (a sidebar with an inline figure containing a caption
/// frame); 4 leaves headroom while still bounding pathological docs.
pub(super) const MAX_ANCHORED_STORY_RECURSION: u32 = 4;

/// Per-line vertical metrics of the *anchoring* line, in page-local pt,
/// expressed as the y of each named reference relative to the line's
/// baseline. The composer hands these in so the `VerticalReferencePoint`
/// resolver can place an anchored frame against the anchor line's
/// x-height / cap-height / leading-top instead of degenerating every
/// such reference to the baseline. Y grows downward, so each "above the
/// baseline" reference is the baseline minus a positive distance.
#[derive(Debug, Clone, Copy)]
pub(super) struct LineRefMetrics {
    /// `LineXHeight` — top of the lowercase x, `baseline - x_height·pt`.
    pub x_height_y: f32,
    /// `LineCapHeight` — top of the capitals, `baseline - cap_height·pt`.
    pub cap_height_y: f32,
    /// `TopOfLeading` — top of the line's leading slug,
    /// `baseline - leading_above`, where `leading_above` splits the
    /// line's leading in the font's ascent:descent proportion.
    pub top_of_leading_y: f32,
}

impl LineRefMetrics {
    /// Resolve the three reference y's from the anchor line's baseline,
    /// the head run's point size, its leading (pt), and the head font's
    /// metrics. When a metric isn't surfaced by the font (legacy faces
    /// with no OS/2 cap-height / x-height) the same em-fraction
    /// fallbacks `first_baseline_for_frame` uses keep the placement
    /// sane: cap-height 0.70 em, x-height 0.50 em. The leading split
    /// uses the font's hhea ascent:descent ratio, falling back to a
    /// 0.8/0.2 split (≈ a typical Latin face) when the descender is
    /// absent or degenerate.
    pub(super) fn resolve(
        baseline_y_pt: f32,
        point_size: f32,
        leading_pt: f32,
        metrics: Option<&FontMetrics>,
    ) -> Self {
        const CAP_HEIGHT_FALLBACK: f32 = 0.70;
        const X_HEIGHT_FALLBACK: f32 = 0.50;
        let cap = metrics
            .and_then(|m| m.cap_height)
            .unwrap_or(CAP_HEIGHT_FALLBACK);
        let xh = metrics
            .and_then(|m| m.x_height)
            .unwrap_or(X_HEIGHT_FALLBACK);
        let ascender = metrics.map(|m| m.ascender).unwrap_or(0.8);
        let descender = metrics.map(|m| m.descender).unwrap_or(0.2);
        // Split the leading in the font's ascent:descent proportion —
        // InDesign's leading model. `leading_above` is the slice of the
        // leading slug that sits above the baseline. Degenerate metrics
        // (ascender+descender ≤ 0) fall back to an 80/20 split.
        let sum = ascender + descender;
        let above_fraction = if sum > 0.0 { ascender / sum } else { 0.8 };
        let leading_above = leading_pt * above_fraction;
        Self {
            x_height_y: baseline_y_pt - xh * point_size,
            cap_height_y: baseline_y_pt - cap * point_size,
            top_of_leading_y: baseline_y_pt - leading_above,
        }
    }
}

/// The host page's margin box, projected into page-local pt. Resolved
/// from the W0.6 `<MarginPreference>` side map (`Spread::page_margins`,
/// keyed by the page's `Self` id). `None` when the page declared no
/// margins — the `PageMargins` reference then degenerates to the page
/// edge, the pre-W1.16 behaviour.
#[derive(Debug, Clone, Copy)]
pub(super) struct PageMarginBox {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Best-effort emission of the paragraph's anchored frames. Supports
/// `InlinePosition` (default) by placing the frame at the
/// paragraph's first-baseline anchor offset by `anchor_x_offset` /
/// `anchor_y_offset`. `AbovePosition` puts it above the paragraph's
/// origin; `Custom` honours the offsets verbatim. Unrecognised
/// positions log a TODO and fall through to InlinePosition placement.
///
/// Anchored TextFrames recurse into their story via the document's
/// frame_chain lookup; anchored Rectangles emit through
/// `emit_rectangle_into` if the parser surfaced bounds for them. We
/// don't yet thread images on anchored rectangles; those land when
/// the parser surfaces image_link on AnchoredFrame.
pub(super) fn emit_anchored_frames_for_paragraph(
    em: &mut StoryEmitter,
    paragraph: &paged_parse::Paragraph,
    pages: &mut [BuiltPage],
    line_metrics: LineRefMetrics,
    margin_box: Option<PageMarginBox>,
    _total_stats: &mut PipelineStats,
) {
    let target_page = em.chain_pages[em.frame_idx];
    let frame = em.chain[em.frame_idx];
    let (ox, oy) = pages[target_page].spread_origin;
    let frame_insets = frame.inset_spacing.unwrap_or([0.0; 4]);
    let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
    let para_origin_x = sx - ox + frame_insets[1] + em.column_x_shift_pt;
    let para_origin_y = sy - oy;
    // Paragraph baseline (page-local pt). y_cursor is in 1/64 pt
    // relative to the host frame's inner origin, so convert + add
    // the frame's spread top-left to get a page-local baseline.
    let baseline_y_pt = if em.y_cursor >= 0 {
        para_origin_y + em.y_cursor as f32 / paged_text::shape::ADVANCE_PRECISION
    } else {
        para_origin_y
    };

    for af in &paragraph.anchored_frames {
        let setting = af.setting.as_ref();
        let position = setting
            .and_then(|s| s.anchored_position.as_deref())
            .unwrap_or("InlinePosition");
        let (offset_x, offset_y) = setting
            .map(|s| (s.anchor_x_offset, s.anchor_y_offset))
            .unwrap_or((0.0, 0.0));
        let frame_w = af.bounds.map(|b| b.width()).unwrap_or(0.0);
        let frame_h = af.bounds.map(|b| b.height()).unwrap_or(0.0);
        // Anchor reference point on the frame — the corner / edge the
        // AnchoredObjectSetting offset attaches to (`TopLeftAnchor`,
        // `TopRightAnchor`, `CenterAnchor`, …). For inline frames
        // we resolve the *vertical* component of the anchor point
        // strictly: a Top anchor sits the frame's top on the line
        // baseline, a Bottom anchor sits the frame's bottom on the
        // baseline (the legacy default), Center splits the diff.
        // The horizontal component currently degenerates because we
        // don't yet thread the per-anchor advance offset out of the
        // composer — both `BottomLeftAnchor` and `BottomRightAnchor`
        // place the frame at the column-left edge of the paragraph
        // (real InDesign would shift `BottomRightAnchor` by the
        // anchor character's full advance, which equals the frame's
        // own width when the anchor is the lone character on the
        // line). Once the composer surfaces the U+FFFC advance
        // position the horizontal degenerates collapse — see the
        // TODO below.
        let anchor_point = setting
            .and_then(|s| s.anchor_point.as_deref())
            .unwrap_or("BottomLeftAnchor");
        // Corner-of-frame corrections: how far the frame's top-left
        // must move so the *named* anchor corner lands on the resolved
        // anchor point. Both are pure functions of the frame size and
        // the anchor-point name (see the unit tests in `mod tests`).
        let vertical_corner_dy = anchor_vertical_corner_offset(anchor_point, frame_h);
        // TODO(anchored-position): once paragraph_breaker exposes the
        // anchor character's advance-from-line-start, replace
        // `para_origin_x` with that advance so `InlinePosition` lands
        // at the actual inline position. The horizontal anchor-corner
        // component is deliberately NOT applied to InlinePosition /
        // AbovePosition: their anchor x is still approximated at the
        // column origin (not the true advance), so shifting by the
        // frame's own width would push a Right anchor off the left of
        // the column. It IS applied to Custom / Anchored, where the
        // reference rect gives a real span to align against.
        let (place_x, place_y) = match position {
            "InlinePosition" => {
                if frame_w > 0.0 && frame_h > 0.0 {
                    tracing::debug!(
                        target: "paged_renderer::pipeline",
                        anchor_point,
                        "InlinePosition: anchored at paragraph origin (per-anchor advance offset queued)"
                    );
                }
                // Frame top-left placed so the named anchor corner
                // sits at (paragraph origin x, baseline y) plus the
                // anchor offsets. The horizontal anchor-corner
                // component stays collapsed until paragraph_breaker
                // exposes the per-anchor advance position; the vertical
                // component drives Top vs Bottom anchoring. `offset_x`
                // is honoured verbatim.
                (
                    para_origin_x + offset_x,
                    baseline_y_pt + offset_y - vertical_corner_dy,
                )
            }
            // Both `AbovePosition` and the (newer) `AboveLine` enum
            // value place the frame above the host line; treat them
            // identically until line-by-line vertical resolution lands.
            "AbovePosition" | "AboveLine" => (
                para_origin_x + offset_x,
                para_origin_y + offset_y - vertical_corner_dy,
            ),
            // `Custom` / `Anchored` — honour HorizontalReferencePoint
            // and VerticalReferencePoint with their alignments so the
            // frame anchors against the column / text-frame / page-
            // edge rectangles the IDML declares. IDML 14+ writes
            // `Anchored`; older docs write `Custom`. Treat both the
            // same.
            "Custom" | "Anchored" => {
                let ref_x = horizontal_reference_x(
                    setting,
                    para_origin_x,
                    frame,
                    &pages[target_page],
                    em.column_x_shift_pt,
                    margin_box,
                );
                let ref_y = vertical_reference_y(
                    setting,
                    baseline_y_pt,
                    frame,
                    &pages[target_page],
                    para_origin_y,
                    line_metrics,
                    margin_box,
                );
                // Custom positioning resolves a real reference span, so
                // both corner components are meaningful: a `RightAlign`
                // reference x with a `*RightAnchor` corner snaps the
                // frame's right edge to the reference's right edge.
                // `resolve_custom_anchor_pos` is the pure composition
                // exercised directly by the reference-point unit tests.
                resolve_custom_anchor_pos(
                    ref_x,
                    ref_y,
                    anchor_point,
                    frame_w,
                    frame_h,
                    offset_x,
                    offset_y,
                )
            }
            _ => {
                tracing::debug!(
                    target: "paged_renderer::pipeline",
                    position = position,
                    "unrecognised anchored position; defaulting to InlinePosition"
                );
                (
                    para_origin_x + offset_x,
                    baseline_y_pt + offset_y - vertical_corner_dy,
                )
            }
        };
        emit_one_anchored_frame(em, af, target_page, place_x, place_y, pages);
    }
}

/// Phase 5 — resolve the horizontal reference x for a `Custom`-
/// positioned anchored frame, accounting for `HorizontalReferencePoint`
/// and `HorizontalAlignment`. Returns the page-local pt x that the
/// frame's anchor corner attaches to BEFORE the AnchoredObjectSetting
/// offset and the corner-of-frame correction are applied.
///
/// Supported references:
/// - `AnchorLocation` (default): the anchor character's x. Today we
///   approximate this as the paragraph origin x (the composer doesn't
///   yet surface per-glyph advance; same limitation as InlinePosition).
/// - `ColumnEdge`: the paragraph's column left edge.
/// - `TextFrame`: the host text frame's spread-projected left edge,
///   page-local.
/// - `PageMargins`: the page's margin box left/right, from the W0.6
///   `<MarginPreference>` side map. Falls back to the page edge when the
///   host page declared no margins.
/// - `PageEdge`: the page bounds.
///
/// `HorizontalAlignment` shifts the resolved x by the reference
/// rectangle's width: `LeftAlign` keeps the left, `CenterAlign`
/// centers, `RightAlign` snaps to the right.
fn horizontal_reference_x(
    setting: Option<&paged_parse::AnchoredObjectSetting>,
    para_origin_x: f32,
    frame: &paged_parse::TextFrame,
    page: &BuiltPage,
    column_x_shift_pt: f32,
    margin_box: Option<PageMarginBox>,
) -> f32 {
    let _ = column_x_shift_pt;
    let reference = setting
        .and_then(|s| s.horizontal_reference_point.as_deref())
        .unwrap_or("AnchorLocation");
    let alignment = setting
        .and_then(|s| s.horizontal_alignment.as_deref())
        .unwrap_or("LeftAlign");
    let frame_spread_bounds = transform_bounds(frame.bounds, frame.item_transform);
    let (ref_left, ref_right) = match reference {
        "AnchorLocation" => (para_origin_x, para_origin_x),
        "ColumnEdge" => {
            // Column edges = the paragraph's effective text box; for
            // single-column frames that's the inset-adjusted frame.
            let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
            let (sx, _sy) = (frame_spread_bounds.left, frame_spread_bounds.top);
            let left_page = sx - page.spread_origin.0 + insets[1];
            let width = (frame_spread_bounds.right - frame_spread_bounds.left).max(0.0)
                - insets[1]
                - insets[3];
            (left_page, left_page + width)
        }
        "TextFrame" => {
            let left = frame_spread_bounds.left - page.spread_origin.0;
            let right = frame_spread_bounds.right - page.spread_origin.0;
            (left, right)
        }
        // PageMargins resolves against the parsed margin box; absent
        // margins degenerate to the page edge.
        "PageMargins" => page_margin_span_h(margin_box, page.width_pt),
        "PageEdge" => (0.0, page.width_pt),
        _ => (para_origin_x, para_origin_x),
    };
    align_in_span(ref_left, ref_right, alignment)
}

/// Horizontal `[near, far]` span for the `PageMargins` reference: the
/// margin box's left/right when the page declared margins, else the page
/// edge `[0, page_width]`. Pure so the margins-vs-page-edge divergence is
/// unit-testable without a `BuiltPage`.
fn page_margin_span_h(margin_box: Option<PageMarginBox>, page_width: f32) -> (f32, f32) {
    match margin_box {
        Some(m) => (m.left, m.right),
        None => (0.0, page_width),
    }
}

/// Vertical analogue of [`page_margin_span_h`].
fn page_margin_span_v(margin_box: Option<PageMarginBox>, page_height: f32) -> (f32, f32) {
    match margin_box {
        Some(m) => (m.top, m.bottom),
        None => (0.0, page_height),
    }
}

/// Pure alignment selection within a `[near, far]` reference span,
/// shared by the horizontal and vertical reference resolvers. Maps an
/// IDML `HorizontalAlignment` / `VerticalAlignment` token to the point
/// in the span the frame's anchor corner attaches to:
/// - `CenterAlign` ⇒ the span's midpoint,
/// - `RightAlign` / `BottomAlign` / `AwayFromBindingSide` ⇒ the far edge,
/// - everything else (`LeftAlign`, `TopAlign`, `TextAlign`,
///   `ToBindingSide`, unknown) ⇒ the near edge.
///
/// `BottomAlign` and `RightAlign` are spelled differently in the two
/// axes but both mean "far edge"; accepting both keeps this one
/// function usable for either axis.
fn align_in_span(near: f32, far: f32, alignment: &str) -> f32 {
    match alignment {
        "CenterAlign" => (near + far) * 0.5,
        "RightAlign" | "BottomAlign" | "AwayFromBindingSide" => far,
        _ => near,
    }
}

/// Phase 5 — vertical analogue of [`horizontal_reference_x`]. Maps
/// `VerticalReferencePoint` + `VerticalAlignment` to the y the frame's
/// anchor corner sits against.
///
/// Supported references:
/// - `LineBaseline` (default): the anchor line's baseline.
/// - `LineXHeight`: the anchor line's x-height (top of the lowercase x).
/// - `LineCapHeight`: the anchor line's cap-height (top of the capitals).
/// - `TopOfLeading`: the top of the anchor line's leading slug.
///   All three come from the head font's metrics threaded in as
///   [`LineRefMetrics`]; each is a zero-width span at its y (anchoring
///   against a horizontal text line, not a rectangle).
/// - `Column`: the host text frame's top (≈ column top).
/// - `TextFrame`: the host text frame's top.
/// - `PageMargins`: the page's margin box top/bottom, from the W0.6
///   `<MarginPreference>` side map; falls back to the page edge when the
///   host page declared no margins.
/// - `PageEdge`: page bounds.
fn vertical_reference_y(
    setting: Option<&paged_parse::AnchoredObjectSetting>,
    baseline_y_pt: f32,
    frame: &paged_parse::TextFrame,
    page: &BuiltPage,
    para_origin_y: f32,
    line_metrics: LineRefMetrics,
    margin_box: Option<PageMarginBox>,
) -> f32 {
    let _ = para_origin_y;
    let reference = setting
        .and_then(|s| s.vertical_reference_point.as_deref())
        .unwrap_or("LineBaseline");
    let alignment = setting
        .and_then(|s| s.vertical_alignment.as_deref())
        .unwrap_or("TopAlign");
    let frame_spread_bounds = transform_bounds(frame.bounds, frame.item_transform);
    let (ref_top, ref_bottom) = match reference {
        "LineBaseline" => (baseline_y_pt, baseline_y_pt),
        // The three line-relative references resolve against the anchor
        // line's real metrics now (degenerate spans — a text line has no
        // height to align within, so near == far).
        "LineXHeight" => (line_metrics.x_height_y, line_metrics.x_height_y),
        "LineCapHeight" => (line_metrics.cap_height_y, line_metrics.cap_height_y),
        "TopOfLeading" => (line_metrics.top_of_leading_y, line_metrics.top_of_leading_y),
        "Column" | "TextFrame" => {
            let top = frame_spread_bounds.top - page.spread_origin.1;
            let bottom = frame_spread_bounds.bottom - page.spread_origin.1;
            (top, bottom)
        }
        // PageMargins resolves against the parsed margin box; absent
        // margins degenerate to the page edge.
        "PageMargins" => page_margin_span_v(margin_box, page.height_pt),
        "PageEdge" => (0.0, page.height_pt),
        _ => (baseline_y_pt, baseline_y_pt),
    };
    align_in_span(ref_top, ref_bottom, alignment)
}

/// Vertical offset from an anchored frame's top to the reference
/// edge / center named by `anchor_point`. Returns `0` for any
/// `Top*Anchor` (frame's top at the anchor's y), `h/2` for any
/// `*CenterAnchor`, and `h` for any `Bottom*Anchor` / unknown values
/// (frame's bottom at the anchor's y — the legacy default that
/// matched the original anchored-frame placement).
fn anchor_vertical_corner_offset(anchor_point: &str, h: f32) -> f32 {
    match anchor_point {
        "TopLeftAnchor" | "TopCenterAnchor" | "TopRightAnchor" => 0.0,
        "LeftCenterAnchor" | "CenterAnchor" | "RightCenterAnchor" => h * 0.5,
        // Bottom* and unknown values fall through to the legacy
        // bottom-anchored placement (frame's bottom at the anchor y).
        _ => h,
    }
}

/// Horizontal analogue of [`anchor_vertical_corner_offset`]: the
/// offset from an anchored frame's left edge to the column of the
/// named `anchor_point`. `0` for any `*LeftAnchor` (frame's left at
/// the anchor x), `w/2` for any `*CenterAnchor` / `CenterAnchor`, and
/// `w` for any `*RightAnchor` (frame's right at the anchor x). Unknown
/// values default to `0` (left-anchored) — the conservative choice
/// that leaves the frame at the reference x rather than off to the
/// left of it.
fn anchor_horizontal_corner_offset(anchor_point: &str, w: f32) -> f32 {
    match anchor_point {
        "TopRightAnchor" | "RightCenterAnchor" | "BottomRightAnchor" => w,
        "TopCenterAnchor" | "CenterAnchor" | "BottomCenterAnchor" => w * 0.5,
        // Left* and unknown values keep the frame's left edge on the
        // anchor x.
        _ => 0.0,
    }
}

/// Pure placement math for a `Custom` / `Anchored` frame: given a
/// resolved reference rectangle (in page-local pt), the named anchor
/// corner, the frame's size, and the X/Y offsets, return the frame's
/// top-left. This is exactly the composition the Custom branch of
/// [`emit_anchored_frames_for_paragraph`] performs once the reference
/// rect has been projected and the alignment applied — factored out so
/// the reference-point math is unit-testable without a `BuiltPage` /
/// `TextFrame`.
///
/// `ref_x` / `ref_y` are the post-alignment reference point (the
/// output of [`align_in_span`] on each axis); `anchor_point` selects
/// which corner of the frame attaches there; `(w, h)` is the frame's
/// size; `(offset_x, offset_y)` are the `AnchorXoffset` /
/// `AnchorYoffset` nudges.
fn resolve_custom_anchor_pos(
    ref_x: f32,
    ref_y: f32,
    anchor_point: &str,
    w: f32,
    h: f32,
    offset_x: f32,
    offset_y: f32,
) -> (f32, f32) {
    let dx = anchor_horizontal_corner_offset(anchor_point, w);
    let dy = anchor_vertical_corner_offset(anchor_point, h);
    (ref_x + offset_x - dx, ref_y + offset_y - dy)
}

/// Emit a single anchored frame (or recurse through a Group). Splits
/// out of `emit_anchored_frames_for_paragraph` so anchored Groups can
/// reuse the same placement logic for each child without duplicating
/// the position-resolution preamble.
///
/// `place_x` / `place_y` are the page-local pt coordinates of the
/// frame's top-left as resolved from the AnchoredObjectSetting
/// (InlinePosition / AbovePosition / Custom). For Group children, the
/// caller offsets these by the child's bounds delta within the group.
fn emit_one_anchored_frame(
    em: &mut StoryEmitter,
    af: &paged_parse::AnchoredFrame,
    target_page: usize,
    place_x: f32,
    place_y: f32,
    pages: &mut [BuiltPage],
) {
    let frame_w = af.bounds.map(|b| b.width()).unwrap_or(0.0);
    let frame_h = af.bounds.map(|b| b.height()).unwrap_or(0.0);
    match af.frame_kind {
        paged_parse::AnchoredFrameKind::Rectangle | paged_parse::AnchoredFrameKind::TextFrame => {
            // Rectangles AND TextFrames render the frame's box +
            // fill / stroke through the same `emit_rectangle_into`
            // pipeline used by spread-level Rectangles. TextFrames
            // additionally host a story; the story-recursion layer
            // is queued (anchored.idml's TextFrame variants ship
            // FillColor=Color/Paper which makes the frame visible
            // even without the inner text). The synthesizer below
            // bakes the page-local placement into a Rectangle whose
            // bounds sit in spread coords so `frame_outer_transform`
            // unwinds back to the right page-local position.
            if frame_w > 0.0 && frame_h > 0.0 {
                emit_anchored_rect_via_pipeline(
                    em,
                    af,
                    target_page,
                    place_x,
                    place_y,
                    frame_w,
                    frame_h,
                    pages,
                );
            }
            // Capture image-bearing anchored Rectangles (incl. Group
            // children). Rendering routes through the per-page +
            // decoded-image caches owned by `build_document`, so
            // we record placement here and the post-pass replays via
            // `emit_rectangle_image`. Anchored TextFrames don't carry
            // an `image_link` (the parser only sets it for Rectangles
            // / Groups), but the guard is symmetric for safety.
            if af.image_link.is_some() && frame_w > 0.0 && frame_h > 0.0 {
                em.anchored_image_queue.push(AnchoredImageEmit {
                    target_page,
                    place_x,
                    place_y,
                    width: frame_w,
                    height: frame_h,
                    af: af.clone(),
                });
            }
            if matches!(af.frame_kind, paged_parse::AnchoredFrameKind::TextFrame) {
                if let Some(story_id) = af.parent_story.as_deref() {
                    if frame_w > 0.0 && frame_h > 0.0 {
                        emit_anchored_textframe_story(
                            em,
                            af,
                            story_id,
                            target_page,
                            place_x,
                            place_y,
                            frame_w,
                            frame_h,
                            pages,
                        );
                    }
                }
            }
        }
        paged_parse::AnchoredFrameKind::Group => {
            // Recurse through the group's children. The group's own
            // ItemTransform (typically a pure translate of the form
            // `[1 0 0 1 tx ty]`) shifts every child by `(tx, ty)` in
            // page-local pt. Each child's `bounds.left` /
            // `bounds.top` are relative to the group's inner-coord
            // origin; we offset by the difference between the
            // child's and the group's `bounds` so the children land
            // at the right spot inside the group's placement rect.
            // Image-link emission for Group children is deferred —
            // the per-page image cache lives outside StoryEmitter.
            let (group_tx, group_ty) = af
                .item_transform
                .map(|m| (m[4], m[5]))
                .unwrap_or((0.0, 0.0));
            let (group_bx, group_by) = af.bounds.map(|b| (b.left, b.top)).unwrap_or((0.0, 0.0));
            for child in &af.children {
                // Child's offset within the group's inner coord
                // system is `child.bounds.{left,top} - group.bounds.{left,top}`.
                // Plus the child's own item_transform (translate
                // component) and the group's item_transform.
                let (child_bx, child_by) =
                    child.bounds.map(|b| (b.left, b.top)).unwrap_or((0.0, 0.0));
                let (child_tx, child_ty) = child
                    .item_transform
                    .map(|m| (m[4], m[5]))
                    .unwrap_or((0.0, 0.0));
                let child_place_x = place_x + group_tx + child_tx + (child_bx - group_bx);
                let child_place_y = place_y + group_ty + child_ty + (child_by - group_by);
                emit_one_anchored_frame(
                    em,
                    child,
                    target_page,
                    child_place_x,
                    child_place_y,
                    pages,
                );
            }
        }
    }
}

/// Synthesize a Rectangle for an anchored frame placed at
/// `(place_x, place_y)` page-local pt with size `(w, h)` and route it
/// through `emit_rectangle_into` so fill / stroke / drop-shadow
/// modules emit identically to a spread-level Rectangle. The
/// synthetic Rectangle's bounds sit in spread coords (page-local +
/// spread_origin) so `frame_outer_transform` produces a translate of
/// `-spread_origin` and lands the geometry back on `(place_x, place_y)`.
fn emit_anchored_rect_via_pipeline(
    em: &StoryEmitter,
    af: &paged_parse::AnchoredFrame,
    target_page: usize,
    place_x: f32,
    place_y: f32,
    w: f32,
    h: f32,
    pages: &mut [BuiltPage],
) {
    let (ox, oy) = pages[target_page].spread_origin;
    let bounds = paged_parse::Bounds {
        top: place_y + oy,
        left: place_x + ox,
        bottom: place_y + oy + h,
        right: place_x + ox + w,
    };
    let synthetic = Rectangle {
        self_id: af.self_id.clone(),
        bounds,
        item_transform: None,
        fill_color: af.fill_color.clone(),
        fill_tint: af.fill_tint,
        stroke_color: af.stroke_color.clone(),
        stroke_weight: af.stroke_weight,
        drop_shadow: None,
        stroke_drop_shadow: None,
        // Image emission for anchored Rectangles is deferred — the
        // per-page image cache lives in the pre-pass scope, outside
        // StoryEmitter. The parser still surfaces image_link /
        // image_item_transform on AnchoredFrame so a future renderer
        // pass can pick them up. Today's anchored.idml ships no
        // image-bearing anchored Rectangles.
        image_link: None,
        image_bytes: None,
        image_clip: None,
        has_image_element: false,
        has_inline_pdf: false,
        image_item_transform: None,
        applied_object_style: af.applied_object_style.clone(),
        text_wrap: None,
        frame_fitting: None,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_dash: Vec::new(),
        stroke_alignment: None,
        end_cap: None,
        end_join: None,
        miter_limit: None,
        item_layer: None,
        corner_radius: None,
        corner_option: None,
        corners: Default::default(),
        is_anchored: true,
        opacity: None,
        blend_mode: None,
        effects: None,
        gradient_fill_angle: af.gradient_fill_angle,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        text_paths: Vec::new(),
        // Anchored frames don't currently carry overprint attrs in our
        // AnchoredFrame mirror; default to knockout (the IDML default).
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
    };
    // `emit_rectangle_into` increments `page.stats.frames` internally.
    emit_rectangle_into(
        &mut pages[target_page],
        &synthetic,
        em.document,
        em.palette,
        em.options.fallback_frame_fill,
        em.cmyk_xform,
        None,
    );
}

/// Image-emit pass for an anchored Rectangle whose `image_link` is
/// populated. Synthesises a Rectangle in spread coords (mirroring
/// `emit_anchored_rect_via_pipeline`'s placement math, plus the
/// image fields the parent helper drops) and hands it to
/// `emit_rectangle_image` so the per-page + decoded-image caches in
/// `build_document`'s scope are reused.
///
/// The image stamps *on top* of the rectangle's own fill / stroke
/// emitted earlier by the body / master story pass — same z-order
/// as a spread-level Rectangle whose `<Image>` child overlays the
/// rectangle's solid fill.
pub(super) fn emit_anchored_rect_image(
    page: &mut BuiltPage,
    af: &paged_parse::AnchoredFrame,
    place_x: f32,
    place_y: f32,
    w: f32,
    h: f32,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, paged_compose::ImageId>,
    decoded_cache: &mut HashMap<String, paged_compose::DecodedImage>,
) {
    let (ox, oy) = page.spread_origin;
    let bounds = paged_parse::Bounds {
        top: place_y + oy,
        left: place_x + ox,
        bottom: place_y + oy + h,
        right: place_x + ox + w,
    };
    let synthetic = Rectangle {
        self_id: af.self_id.clone(),
        bounds,
        item_transform: None,
        fill_color: af.fill_color.clone(),
        fill_tint: af.fill_tint,
        stroke_color: af.stroke_color.clone(),
        stroke_weight: af.stroke_weight,
        drop_shadow: None,
        stroke_drop_shadow: None,
        image_link: af.image_link.clone(),
        has_image_element: af.image_link.is_some(),
        has_inline_pdf: false,
        image_item_transform: af.image_item_transform,
        image_bytes: None,
        image_clip: None,
        applied_object_style: af.applied_object_style.clone(),
        text_wrap: None,
        frame_fitting: None,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_dash: Vec::new(),
        stroke_alignment: None,
        end_cap: None,
        end_join: None,
        miter_limit: None,
        item_layer: None,
        corner_radius: None,
        corner_option: None,
        corners: Default::default(),
        is_anchored: true,
        opacity: None,
        blend_mode: None,
        effects: None,
        gradient_fill_angle: af.gradient_fill_angle,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        text_paths: Vec::new(),
        // Anchored frames don't currently carry overprint attrs in our
        // AnchoredFrame mirror; default to knockout (the IDML default).
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
    };
    emit_rectangle_image(page, &synthetic, options, page_image_cache, decoded_cache);
}

/// Flow the story referenced by an anchored TextFrame into the
/// placed rectangle. Synthesises a single-frame chain whose `bounds`
/// sit in spread coords (so `frame_outer_transform` produces a
/// `-spread_origin` translate that lands the geometry on
/// `(place_x, place_y)`), then runs the existing per-paragraph emit
/// loop on a fresh sub-`StoryEmitter`. The sub-emitter inherits the
/// parent's document / palette / font_table / cmyk / hyphenator
/// borrows so no extra plumbing is needed.
///
/// Recursion is bounded by [`MAX_ANCHORED_STORY_RECURSION`]: an
/// anchored TextFrame inside an anchored TextFrame is fine, but a
/// pathological cycle (anchored TextFrame whose story re-references
/// itself) is short-circuited with a `tracing::warn!`.
///
/// Inset spacing on the synthetic frame is `[0; 4]` because parsed
/// `AnchoredFrame` records don't carry `<TextFramePreference
/// InsetSpacing>` — anchored frames in real-world IDMLs typically
/// rely on the ObjectStyle cascade for insets, which the renderer's
/// `emit_text_frame_into` pre-pass already drew the box from. The
/// inner story flows edge-to-edge inside the frame's bounds.
pub(super) fn emit_anchored_textframe_story<'a>(
    em: &mut StoryEmitter<'a>,
    af: &paged_parse::AnchoredFrame,
    story_id: &str,
    target_page: usize,
    place_x: f32,
    place_y: f32,
    w: f32,
    h: f32,
    pages: &mut [BuiltPage],
) {
    if em.anchored_recursion_depth >= MAX_ANCHORED_STORY_RECURSION {
        tracing::warn!(
            target: "paged_renderer::pipeline",
            depth = em.anchored_recursion_depth,
            story_id = story_id,
            "anchored TextFrame recursion depth cap hit; skipping inner story"
        );
        return;
    }
    let Some(parsed) = em.document.stories.iter().find(|s| s.self_id == story_id) else {
        return;
    };
    // Build the synthetic TextFrame's bounds in spread coords. The
    // sub-emitter's per-line walk transforms `bounds` through the
    // (None) item_transform and subtracts the page's spread_origin —
    // the shape of `emit_anchored_rect_via_pipeline` for fill /
    // stroke, but here driving the StoryEmitter rather than
    // `emit_rectangle_into`.
    let (ox, oy) = pages[target_page].spread_origin;
    let bounds = paged_parse::Bounds {
        top: place_y + oy,
        left: place_x + ox,
        bottom: place_y + oy + h,
        right: place_x + ox + w,
    };
    let synthetic = TextFrame {
        self_id: af.self_id.clone(),
        parent_story: Some(story_id.to_string()),
        bounds,
        item_transform: None,
        fill_color: None,
        fill_tint: None,
        stroke_color: None,
        stroke_weight: None,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_dash: Vec::new(),
        drop_shadow: None,
        stroke_drop_shadow: None,
        next_text_frame: None,
        vertical_justification: None,
        first_baseline_offset: None,
        minimum_first_baseline_offset: None,
        inset_spacing: None,
        auto_sizing: None,
        auto_sizing_reference_point: None,
        minimum_width_for_auto_sizing: None,
        minimum_height_for_auto_sizing: None,
        use_minimum_height_for_auto_sizing: None,
        column_count: None,
        column_gutter: None,
        column_balance: None,
        applied_object_style: af.applied_object_style.clone(),
        text_wrap: None,
        item_layer: None,
        is_anchored: true,
        opacity: None,
        blend_mode: None,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
        effects: None,
        gradient_fill_angle: None,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        applied_toc_style: None,
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
    };
    // Sub-emitter borrows from the parent's `'a` so the document /
    // palette / font_table refs share lifetimes with the body pass.
    // The synthetic frame lives on this stack frame; the sub-emitter
    // is dropped before this function returns, so the chain's
    // `&TextFrame` borrow is sound.
    let chain: Vec<&TextFrame> = vec![&synthetic];
    let chain_pages: Vec<usize> = vec![target_page];
    let head_wrap_rects: &[WrapShape] = &[];
    let chain_wrap_rects: Vec<&[WrapShape]> = vec![&[]];
    let mut sub = StoryEmitter::new(
        em.document,
        em.options,
        em.palette,
        em.cmyk_xform,
        em.font_table,
        chain,
        chain_pages,
        em.page_labels,
        em.hyphenator,
        head_wrap_rects,
        chain_wrap_rects,
    )
    .with_optical_margin(
        parsed.story.optical_margin_alignment,
        parsed.story.optical_margin_size,
    )
    .with_anchored_recursion_depth(em.anchored_recursion_depth + 1);
    // The story-pass entry point uses a fresh PipelineStats per call
    // for stat aggregation; we accumulate into a discard local rather
    // than the document-wide `total_stats` because anchored stories
    // already counted into `frames` via the synthetic-rect emission.
    // The user-visible counters that matter (paragraphs / runs /
    // glyphs) get added to the page stats by the body emit functions
    // directly.
    let mut sub_stats = PipelineStats::default();
    for paragraph in &parsed.story.paragraphs {
        sub.emit_paragraph(paragraph, pages, &mut sub_stats);
    }
    sub.apply_vertical_justification(pages);
    sub.apply_blend_groups(pages);
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    // --- align_in_span: the shared L/C/R (and T/C/B) selector ----------

    #[test]
    fn align_in_span_picks_near_center_far() {
        // Span [100, 180] (width 80).
        assert!(close(align_in_span(100.0, 180.0, "LeftAlign"), 100.0));
        assert!(close(align_in_span(100.0, 180.0, "TopAlign"), 100.0));
        assert!(close(align_in_span(100.0, 180.0, "CenterAlign"), 140.0));
        assert!(close(align_in_span(100.0, 180.0, "RightAlign"), 180.0));
        assert!(close(align_in_span(100.0, 180.0, "BottomAlign"), 180.0));
        assert!(close(
            align_in_span(100.0, 180.0, "AwayFromBindingSide"),
            180.0
        ));
        // Unknown / binding-side-toward ⇒ near edge.
        assert!(close(align_in_span(100.0, 180.0, "ToBindingSide"), 100.0));
        assert!(close(align_in_span(100.0, 180.0, "Whatever"), 100.0));
    }

    // --- corner offsets: frame-local correction for the anchor point ---

    #[test]
    fn horizontal_corner_offset_left_center_right() {
        let w = 60.0;
        assert!(close(
            anchor_horizontal_corner_offset("TopLeftAnchor", w),
            0.0
        ));
        assert!(close(
            anchor_horizontal_corner_offset("BottomLeftAnchor", w),
            0.0
        ));
        assert!(close(
            anchor_horizontal_corner_offset("TopCenterAnchor", w),
            30.0
        ));
        assert!(close(
            anchor_horizontal_corner_offset("CenterAnchor", w),
            30.0
        ));
        assert!(close(
            anchor_horizontal_corner_offset("TopRightAnchor", w),
            60.0
        ));
        assert!(close(
            anchor_horizontal_corner_offset("BottomRightAnchor", w),
            60.0
        ));
        // Unknown ⇒ left (0), the conservative default.
        assert!(close(anchor_horizontal_corner_offset("Mystery", w), 0.0));
    }

    #[test]
    fn vertical_corner_offset_top_center_bottom() {
        let h = 36.0;
        assert!(close(
            anchor_vertical_corner_offset("TopLeftAnchor", h),
            0.0
        ));
        assert!(close(
            anchor_vertical_corner_offset("TopRightAnchor", h),
            0.0
        ));
        assert!(close(
            anchor_vertical_corner_offset("CenterAnchor", h),
            18.0
        ));
        assert!(close(
            anchor_vertical_corner_offset("RightCenterAnchor", h),
            18.0
        ));
        // Bottom* and unknown ⇒ full height (legacy bottom-anchored).
        assert!(close(
            anchor_vertical_corner_offset("BottomLeftAnchor", h),
            36.0
        ));
        assert!(close(anchor_vertical_corner_offset("Mystery", h), 36.0));
    }

    // --- resolve_custom_anchor_pos: reference rect → frame top-left ----
    //
    // The reference span used below models a 80×120 pt reference rect
    // whose top-left is (100, 200); each case picks the post-alignment
    // reference point with `align_in_span` and asserts the resulting
    // frame top-left for a 60×36 pt frame. ≥6 reference / anchor combos.

    /// Reference rect helper: left/right and top/bottom of the span.
    const REF_L: f32 = 100.0;
    const REF_R: f32 = 180.0; // 80 wide
    const REF_T: f32 = 200.0;
    const REF_B: f32 = 320.0; // 120 tall
    const FRAME_W: f32 = 60.0;
    const FRAME_H: f32 = 36.0;

    #[test]
    fn custom_top_left_anchor_left_top_align_no_offset() {
        // 1) TopLeft anchor, Left/Top alignment ⇒ frame top-left sits
        //    exactly on the reference's top-left.
        let rx = align_in_span(REF_L, REF_R, "LeftAlign");
        let ry = align_in_span(REF_T, REF_B, "TopAlign");
        let (x, y) = resolve_custom_anchor_pos(rx, ry, "TopLeftAnchor", FRAME_W, FRAME_H, 0.0, 0.0);
        assert!(close(x, 100.0) && close(y, 200.0), "got ({x}, {y})");
    }

    #[test]
    fn custom_top_left_anchor_with_offsets() {
        // 2) Same anchor, with +24/+12 offsets (the gen custom_offset
        //    case) ⇒ shifted by the offsets.
        let rx = align_in_span(REF_L, REF_R, "LeftAlign");
        let ry = align_in_span(REF_T, REF_B, "TopAlign");
        let (x, y) =
            resolve_custom_anchor_pos(rx, ry, "TopLeftAnchor", FRAME_W, FRAME_H, 24.0, 12.0);
        assert!(close(x, 124.0) && close(y, 212.0), "got ({x}, {y})");
    }

    #[test]
    fn custom_top_right_anchor_right_align() {
        // 3) TopRight anchor + RightAlign ⇒ frame's right edge on the
        //    reference's right edge: x = 180 - 60 = 120; top at ref top.
        let rx = align_in_span(REF_L, REF_R, "RightAlign");
        let ry = align_in_span(REF_T, REF_B, "TopAlign");
        let (x, y) =
            resolve_custom_anchor_pos(rx, ry, "TopRightAnchor", FRAME_W, FRAME_H, 0.0, 0.0);
        assert!(close(x, 120.0) && close(y, 200.0), "got ({x}, {y})");
    }

    #[test]
    fn custom_center_anchor_center_align() {
        // 4) Center anchor + CenterAlign on both axes ⇒ frame centred
        //    on the reference centre. ref centre = (140, 260); frame
        //    top-left = centre - (w/2, h/2) = (140-30, 260-18).
        let rx = align_in_span(REF_L, REF_R, "CenterAlign");
        let ry = align_in_span(REF_T, REF_B, "CenterAlign");
        let (x, y) = resolve_custom_anchor_pos(rx, ry, "CenterAnchor", FRAME_W, FRAME_H, 0.0, 0.0);
        assert!(close(x, 110.0) && close(y, 242.0), "got ({x}, {y})");
    }

    #[test]
    fn custom_bottom_right_anchor_right_bottom_align() {
        // 5) BottomRight anchor + Right/Bottom alignment ⇒ frame's
        //    bottom-right corner on the reference's bottom-right corner:
        //    x = 180 - 60 = 120; y = 320 - 36 = 284.
        let rx = align_in_span(REF_L, REF_R, "RightAlign");
        let ry = align_in_span(REF_T, REF_B, "BottomAlign");
        let (x, y) =
            resolve_custom_anchor_pos(rx, ry, "BottomRightAnchor", FRAME_W, FRAME_H, 0.0, 0.0);
        assert!(close(x, 120.0) && close(y, 284.0), "got ({x}, {y})");
    }

    #[test]
    fn custom_bottom_left_anchor_bottom_align_default_corner() {
        // 6) BottomLeft anchor (default) + Bottom alignment ⇒ frame's
        //    bottom-left corner on the reference's bottom-left: x stays
        //    at ref left (100), y = 320 - 36 = 284.
        let rx = align_in_span(REF_L, REF_R, "LeftAlign");
        let ry = align_in_span(REF_T, REF_B, "BottomAlign");
        let (x, y) =
            resolve_custom_anchor_pos(rx, ry, "BottomLeftAnchor", FRAME_W, FRAME_H, 0.0, 0.0);
        assert!(close(x, 100.0) && close(y, 284.0), "got ({x}, {y})");
    }

    #[test]
    fn custom_top_center_anchor_center_align_horizontal_only() {
        // 7) TopCenter anchor + CenterAlign horizontal, TopAlign
        //    vertical ⇒ frame horizontally centred on ref centre x,
        //    top on ref top. centre x = 140; frame left = 140 - 30 = 110.
        let rx = align_in_span(REF_L, REF_R, "CenterAlign");
        let ry = align_in_span(REF_T, REF_B, "TopAlign");
        let (x, y) =
            resolve_custom_anchor_pos(rx, ry, "TopCenterAnchor", FRAME_W, FRAME_H, 0.0, 0.0);
        assert!(close(x, 110.0) && close(y, 200.0), "got ({x}, {y})");
    }

    // --- LineRefMetrics: x-height / cap-height / top-of-leading -------
    //
    // A known test face: cap-height 0.700, x-height 0.520, ascender
    // 0.800, descender 0.200 (em-fractions). These are the kind of
    // values an OS/2 v2 + hhea table exposes. Point size 20 pt, leading
    // 24 pt (120% auto), baseline at page-local y = 500.

    fn test_face() -> FontMetrics {
        FontMetrics {
            cap_height: Some(0.700),
            x_height: Some(0.520),
            ascender: 0.800,
            descender: 0.200,
        }
    }

    const BASELINE_Y: f32 = 500.0;
    const PT: f32 = 20.0;
    const LEADING: f32 = 24.0;

    #[test]
    fn line_x_height_is_baseline_minus_xheight_times_pt() {
        let m = test_face();
        let lm = LineRefMetrics::resolve(BASELINE_Y, PT, LEADING, Some(&m));
        // x-height top sits 0.520 × 20 = 10.4 pt above the baseline.
        assert!(close(lm.x_height_y, 500.0 - 10.4), "got {}", lm.x_height_y);
    }

    #[test]
    fn line_cap_height_is_baseline_minus_capheight_times_pt() {
        let m = test_face();
        let lm = LineRefMetrics::resolve(BASELINE_Y, PT, LEADING, Some(&m));
        // cap-height top sits 0.700 × 20 = 14.0 pt above the baseline.
        assert!(
            close(lm.cap_height_y, 500.0 - 14.0),
            "got {}",
            lm.cap_height_y
        );
    }

    #[test]
    fn top_of_leading_splits_leading_by_ascent_descent_ratio() {
        let m = test_face();
        let lm = LineRefMetrics::resolve(BASELINE_Y, PT, LEADING, Some(&m));
        // above-fraction = 0.800 / (0.800 + 0.200) = 0.8; leading_above
        // = 24 × 0.8 = 19.2 pt above the baseline.
        assert!(
            close(lm.top_of_leading_y, 500.0 - 19.2),
            "got {}",
            lm.top_of_leading_y
        );
        // The three references are strictly ordered above the baseline:
        // top-of-leading is highest, then cap-height, then x-height.
        assert!(lm.top_of_leading_y < lm.cap_height_y);
        assert!(lm.cap_height_y < lm.x_height_y);
        assert!(lm.x_height_y < BASELINE_Y);
    }

    #[test]
    fn line_metrics_fall_back_when_face_lacks_os2_cap_xheight() {
        // A legacy face that exposes no OS/2 cap-height / x-height: the
        // resolver falls back to 0.70 em cap, 0.50 em x (the same
        // fallbacks `first_baseline_for_frame` uses). Descender present
        // so the leading split still uses the real ratio.
        let m = FontMetrics {
            cap_height: None,
            x_height: None,
            ascender: 0.750,
            descender: 0.250,
        };
        let lm = LineRefMetrics::resolve(BASELINE_Y, PT, LEADING, Some(&m));
        assert!(
            close(lm.cap_height_y, 500.0 - 0.70 * PT),
            "got {}",
            lm.cap_height_y
        );
        assert!(
            close(lm.x_height_y, 500.0 - 0.50 * PT),
            "got {}",
            lm.x_height_y
        );
        // above-fraction = 0.75 / 1.0 = 0.75; leading_above = 24 × 0.75 = 18.
        assert!(
            close(lm.top_of_leading_y, 500.0 - 18.0),
            "got {}",
            lm.top_of_leading_y
        );
    }

    #[test]
    fn line_metrics_no_face_uses_full_fallback() {
        // No metrics at all (font_id missed in the metrics map): cap
        // 0.70, x 0.50, leading split 0.8/0.2.
        let lm = LineRefMetrics::resolve(BASELINE_Y, PT, LEADING, None);
        assert!(close(lm.cap_height_y, 500.0 - 0.70 * PT));
        assert!(close(lm.x_height_y, 500.0 - 0.50 * PT));
        assert!(close(lm.top_of_leading_y, 500.0 - 0.8 * LEADING));
    }

    #[test]
    fn top_of_leading_handles_degenerate_metrics() {
        // ascender + descender == 0 ⇒ fall back to the 0.8 split rather
        // than dividing by zero.
        let m = FontMetrics {
            cap_height: Some(0.7),
            x_height: Some(0.5),
            ascender: 0.0,
            descender: 0.0,
        };
        let lm = LineRefMetrics::resolve(BASELINE_Y, PT, LEADING, Some(&m));
        assert!(lm.top_of_leading_y.is_finite());
        assert!(close(lm.top_of_leading_y, 500.0 - 0.8 * LEADING));
    }

    // --- PageMargins vs PageEdge span divergence ----------------------

    const PAGE_W: f32 = 595.276;
    const PAGE_H: f32 = 841.890;

    fn margin_box() -> PageMarginBox {
        // 36 pt top, 48 bottom, 54 left, 54 right → page-local box.
        PageMarginBox {
            left: 54.0,
            top: 36.0,
            right: PAGE_W - 54.0,
            bottom: PAGE_H - 48.0,
        }
    }

    #[test]
    fn page_margins_h_span_uses_margin_box_when_present() {
        let (near, far) = page_margin_span_h(Some(margin_box()), PAGE_W);
        assert!(
            close(near, 54.0) && close(far, PAGE_W - 54.0),
            "got ({near}, {far})"
        );
        // RightAlign snaps to the margin's right edge, NOT the page edge.
        assert!(close(align_in_span(near, far, "RightAlign"), PAGE_W - 54.0));
    }

    #[test]
    fn page_margins_h_span_degenerates_to_page_edge_when_absent() {
        let (near, far) = page_margin_span_h(None, PAGE_W);
        assert!(
            close(near, 0.0) && close(far, PAGE_W),
            "got ({near}, {far})"
        );
    }

    #[test]
    fn page_margins_v_span_uses_margin_box_when_present() {
        let (near, far) = page_margin_span_v(Some(margin_box()), PAGE_H);
        assert!(
            close(near, 36.0) && close(far, PAGE_H - 48.0),
            "got ({near}, {far})"
        );
        assert!(close(
            align_in_span(near, far, "BottomAlign"),
            PAGE_H - 48.0
        ));
    }

    #[test]
    fn page_margins_v_span_degenerates_to_page_edge_when_absent() {
        let (near, far) = page_margin_span_v(None, PAGE_H);
        assert!(
            close(near, 0.0) && close(far, PAGE_H),
            "got ({near}, {far})"
        );
    }

    #[test]
    fn page_margins_diverge_from_page_edge() {
        // The whole point of W1.16's margin wire-up: with margins parsed,
        // a `PageMargins` LeftAlign lands at the margin's left (54), NOT
        // at the page edge (0) that `PageEdge` would give.
        let (m_near, _) = page_margin_span_h(Some(margin_box()), PAGE_W);
        let (e_near, _) = page_margin_span_h(None, PAGE_W);
        assert!(
            !close(m_near, e_near),
            "margins must diverge from page edge"
        );
        assert!(close(m_near, 54.0) && close(e_near, 0.0));
    }
}
