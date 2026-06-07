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

//! Flattened renderer IR for page items.
//!
//! Every page-item kind (`TextFrame`, `Rectangle`, `Oval`, `Polygon`,
//! `GraphicLine`) lands in [`ResolvedFrame`] before emission. Modules
//! never name the parser shape types — they read fields off the
//! resolved view and emit through the geometry adapter.

use paged_compose::{BlendMode, Paint, PathId, Rect, Transform};
use paged_parse::{
    DropShadowSetting, GraphicLine, Oval, PathAnchor, Polygon, Rectangle, TextFrame,
};

use crate::pipeline::BuiltPage;

/// Cross-cutting state of a page item, flattened from the parser
/// shape structs. Built once by the per-shape adapters at the top of
/// the emit pipeline. Lifetimes borrow from the parser struct that
/// produced this view; the resolved frame outlives no allocations.
///
/// Some fields are not consumed by any module yet (e.g.
/// `corner_radius`, `applied_object_style`); they're populated up-front
/// so the modules introduced in later commits can read them without
/// touching adapters again.
#[allow(dead_code)]
pub(crate) struct ResolvedFrame<'a> {
    pub self_id: Option<&'a str>,
    pub item_transform: Option<[f32; 6]>,
    pub fill_color: Option<&'a str>,
    pub stroke_color: Option<&'a str>,
    /// `None` when neither the frame nor (after cascade) the
    /// applied object style carry a `StrokeWeight`. Modules apply
    /// the per-frame default of 1.0 via [`Self::effective_stroke_weight`].
    pub stroke_weight: Option<f32>,
    pub fill_tint: Option<f32>,
    pub opacity: Option<f32>,
    /// Already mapped through `blend_mode_from_idml` at adapter time.
    pub blend_mode: BlendMode,
    pub gradient_fill_angle: Option<f32>,
    /// `GradientFillLength` in pt — page-space length of the gradient
    /// line through the frame centre. `None` ⇒ bbox diagonal.
    pub gradient_fill_length: Option<f32>,
    /// `GradientStrokeAngle` in degrees — same convention as
    /// `gradient_fill_angle`, applied to the stroke gradient.
    pub gradient_stroke_angle: Option<f32>,
    /// `GradientStrokeLength` in pt — counterpart to
    /// `gradient_fill_length` for the stroke.
    pub gradient_stroke_length: Option<f32>,
    pub drop_shadow: Option<&'a DropShadowSetting>,
    pub stroke_alignment: Option<&'a str>,
    pub stroke_type: Option<&'a str>,
    pub end_cap: Option<&'a str>,
    pub end_join: Option<&'a str>,
    pub miter_limit: Option<f32>,
    /// FINDING #7.5 — the frame's INSTANCE `GapColor` / `GapTint`
    /// (W0.3 mutation target). Takes precedence over the
    /// `StrokeStyleDef`'s gap colour when painting dashed/dotted/striped
    /// gaps. `None` ⇒ fall back to the style def's gap colour.
    pub stroke_gap_color: Option<&'a str>,
    pub stroke_gap_tint: Option<f32>,
    /// W1.1 — the frame's INSTANCE `StrokeDashAndGap` override (the
    /// per-frame dash mutation target). When non-empty it takes
    /// PRECEDENCE over the `StrokeStyleDef` pattern / built-in name at
    /// paint time — the same instance-wins precedent as
    /// `stroke_gap_color` (FINDING #7.5). Empty slice ⇒ fall back to
    /// the stroke style.
    pub stroke_dash: &'a [f32],
    pub corner_radius: Option<f32>,
    pub corner_option: Option<&'a str>,
    /// Q-16: per-corner `(option, radius)` overrides. When any corner
    /// is non-empty, `corner_path_module` produces a 4-corner path
    /// instead of the symmetric one driven by `corner_radius` /
    /// `corner_option`.
    pub corners: [paged_parse::CornerSpec; 4],
    pub applied_object_style: Option<&'a str>,
    /// `OverprintFill="true"` on the source shape. Flagged at adapter
    /// time so the fill module can route its emit through
    /// [`paged_compose::DisplayCommand::FillPathOverprint`] instead of
    /// the knockout `FillPath`.
    pub overprint_fill: bool,
    /// `OverprintStroke="true"` analogue. Drives the stroke module's
    /// choice between [`paged_compose::DisplayCommand::StrokePath`] and
    /// [`paged_compose::DisplayCommand::StrokePathOverprint`].
    pub overprint_stroke: bool,
    pub geometry: Geometry<'a>,
}

/// Per-shape geometry. Modules that need to emit a primitive consult
/// this enum through [`super::geometry`] adapters; modules that only
/// read cross-cutting state ignore it.
#[allow(dead_code)]
pub(crate) enum Geometry<'a> {
    Rect {
        rect: Rect,
    },
    Oval {
        rect: Rect,
    },
    Line {
        p0: (f32, f32),
        p1: (f32, f32),
    },
    Polygon {
        anchors: &'a [PathAnchor],
        /// Start indices of `<GeometryPathType>` contours within
        /// `anchors`. Empty slice means "one contour" (the legacy
        /// serialisation); multiple entries mark compound paths so
        /// the renderer emits one MoveTo/Close per subpath rather
        /// than joining them.
        subpath_starts: &'a [usize],
        /// Parallel to `subpath_starts`. `true` ⇒ that contour is
        /// open (skip auto-close + final cubic). Empty slice =
        /// every contour closed (the legacy default). For single-
        /// contour shapes with an open path, this is a 1-element
        /// vec carrying `true`, even though `subpath_starts` stays
        /// empty (P-15).
        subpath_open: &'a [bool],
        bbox: Rect,
    },
    /// TextFrames render as rectangles today; carrying a distinct
    /// variant lets the geometry adapter add path-shaped clipping
    /// later without touching modules.
    TextFrameRect {
        rect: Rect,
    },
}

/// Mutable scratch passed to every module. Holds the page's display
/// list, the resolved palette / colour-space transform, the
/// fallback paints from `PipelineOptions`, and a couple of slots
/// that earlier modules use to communicate with later ones (e.g.
/// `corner_path_module` interns a rounded-rect path that
/// `fill_paint_module` then fills). Constructed in Commit 3 — kept
/// here as the design contract for the migration in flight.
#[allow(dead_code)]
pub(crate) struct RenderCtx<'a> {
    pub page: &'a mut BuiltPage,
    pub palette: &'a paged_parse::Graphic,
    pub cmyk_xform: Option<&'a paged_color::IccTransform>,
    pub fallback_paint: Paint,
    pub fallback_drop_shadow: Option<paged_compose::DropShadow>,
    /// Composed `spread_origin × ItemTransform`; used by every paint
    /// emit so the math runs once per frame.
    pub outer: Transform,
    /// Set by `corner_path_module` (rounded Rectangle) so the fill
    /// module emits `FillPath` against the rounded path instead of
    /// a unit-rect-scaled axis-aligned rect.
    pub fill_path: Option<PathId>,
    /// Set by `corner_path_module` (with stroke alignment baked in)
    /// so the stroke module strokes the offset path.
    pub stroke_path: Option<PathId>,
}

fn rect_from_bounds(b: paged_parse::Bounds) -> Rect {
    Rect {
        x: b.left,
        y: b.top,
        w: b.width(),
        h: b.height(),
    }
}

/// True when a TextFrame's parsed `<PathGeometry>` is the ordinary
/// axis-aligned rectangle that `TextFrameRect` already paints — so the
/// frame stays on the cheap rect emitter. The frame is treated as a
/// plain rect when:
///
///   * it carries no anchors (synthetic `GeometricBounds`-only IDML), or
///   * it is a single contour of exactly four anchors whose `anchor`
///     points collapse to two unique x values and two unique y values
///     (an axis-aligned box) and whose Bezier handles coincide with
///     their anchors (no rounded / curved sides).
///
/// Anything else — a triangle, a pentagon, curved sides, an explicitly
/// open path, or a compound multi-contour path — is a real shape and
/// must paint through `Geometry::Polygon`. Mirrors the inner-coord
/// rectangularity test in `pipeline::frame_polygon_spread` (which gates
/// the parallel text-layout clip) so the paint and the clip agree on
/// what counts as "rectangular".
fn text_frame_is_rect_path(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
) -> bool {
    if anchors.is_empty() {
        return true;
    }
    // A compound or explicitly-open path is never a plain rect.
    if subpath_starts.len() > 1 || subpath_open.iter().any(|&o| o) {
        return false;
    }
    if anchors.len() != 4 {
        return false;
    }
    let eq = |a: f32, b: f32| (a - b).abs() < 1e-3;
    // Curved sides: any handle that doesn't sit on its own anchor means
    // the outline bows away from the bounding box.
    for a in anchors {
        if !eq(a.left.0, a.anchor.0)
            || !eq(a.left.1, a.anchor.1)
            || !eq(a.right.0, a.anchor.0)
            || !eq(a.right.1, a.anchor.1)
        {
            return false;
        }
    }
    let mut xs: Vec<f32> = anchors.iter().map(|a| a.anchor.0).collect();
    let mut ys: Vec<f32> = anchors.iter().map(|a| a.anchor.1).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    eq(xs[0], xs[1]) && eq(xs[2], xs[3]) && eq(ys[0], ys[1]) && eq(ys[2], ys[3])
}

impl<'a> ResolvedFrame<'a> {
    /// Stroke weight with InDesign's per-frame default applied
    /// (`1.0` pt). Modules use this when emitting; the `Option`
    /// shape on the field exists only so the object-style cascade
    /// can distinguish "frame had no StrokeWeight" from "frame had
    /// StrokeWeight=1.0".
    pub(crate) fn effective_stroke_weight(&self) -> f32 {
        self.stroke_weight.unwrap_or(1.0)
    }

    pub(crate) fn from_text_frame(frame: &'a TextFrame) -> Self {
        // W1.1: a TextFrame whose `<PathGeometry>` is genuinely non-
        // rectangular (triangle, pentagon, Bezier outline, or a
        // compound multi-contour path) paints its *fill / stroke* via
        // the real path rather than the AABB. InDesign serialises a
        // path even for plain rectangles (4 axis-aligned corners), so
        // we only lift when the anchors describe something the
        // `TextFrameRect` primitive can't reproduce — `text_frame_is_
        // rect_path` keeps every ordinary text panel on the cheap rect
        // emitter. Text *layout* still clips to `frame.anchors`
        // independently (see `frame_polygon_spread`); this only affects
        // the frame's own paint.
        let bbox = rect_from_bounds(frame.bounds);
        let geometry = if text_frame_is_rect_path(
            &frame.anchors,
            &frame.subpath_starts,
            &frame.subpath_open,
        ) {
            Geometry::TextFrameRect { rect: bbox }
        } else {
            Geometry::Polygon {
                anchors: &frame.anchors,
                subpath_starts: &frame.subpath_starts,
                subpath_open: &frame.subpath_open,
                bbox,
            }
        };
        Self {
            self_id: frame.self_id.as_deref(),
            item_transform: frame.item_transform,
            fill_color: frame.fill_color.as_deref(),
            stroke_color: frame.stroke_color.as_deref(),
            stroke_weight: frame.stroke_weight,
            fill_tint: frame.fill_tint,
            opacity: frame.opacity,
            blend_mode: crate::pipeline::blend_mode_from_idml(frame.blend_mode.as_deref()),
            gradient_fill_angle: frame.gradient_fill_angle,
            gradient_fill_length: frame.gradient_fill_length,
            gradient_stroke_angle: frame.gradient_stroke_angle,
            gradient_stroke_length: frame.gradient_stroke_length,
            drop_shadow: frame.drop_shadow.as_ref(),
            stroke_alignment: None,
            stroke_type: frame.stroke_type.as_deref(),
            end_cap: None,
            end_join: None,
            miter_limit: None,
            stroke_gap_color: frame.stroke_gap_color.as_deref(),
            stroke_gap_tint: frame.stroke_gap_tint,
            stroke_dash: &frame.stroke_dash,
            corner_radius: None,
            corner_option: None,
            corners: Default::default(),
            applied_object_style: frame.applied_object_style.as_deref(),
            overprint_fill: frame.overprint_fill,
            overprint_stroke: frame.overprint_stroke,
            geometry,
        }
    }

    pub(crate) fn from_rectangle(rect: &'a Rectangle) -> Self {
        // Q-11: a `<Rectangle>` with more than 4 path anchors carries a
        // stylised non-rectangular outline (torn-paper, asymmetric
        // multi-anchor decorations Envato saves as `<Rectangle>` rather
        // than `<Polygon>`). Mirror `from_polygon`'s adapter so paint
        // modules see the real curve instead of collapsing to the AABB.
        let bbox = rect_from_bounds(rect.bounds);
        let geometry = if rect.anchors.len() > 4 {
            Geometry::Polygon {
                anchors: &rect.anchors,
                subpath_starts: &rect.subpath_starts,
                subpath_open: &rect.subpath_open,
                bbox,
            }
        } else {
            Geometry::Rect { rect: bbox }
        };
        Self {
            self_id: rect.self_id.as_deref(),
            item_transform: rect.item_transform,
            fill_color: rect.fill_color.as_deref(),
            stroke_color: rect.stroke_color.as_deref(),
            stroke_weight: rect.stroke_weight,
            fill_tint: rect.fill_tint,
            opacity: rect.opacity,
            blend_mode: crate::pipeline::blend_mode_from_idml(rect.blend_mode.as_deref()),
            gradient_fill_angle: rect.gradient_fill_angle,
            gradient_fill_length: rect.gradient_fill_length,
            gradient_stroke_angle: rect.gradient_stroke_angle,
            gradient_stroke_length: rect.gradient_stroke_length,
            drop_shadow: rect.drop_shadow.as_ref(),
            stroke_alignment: rect.stroke_alignment.as_deref(),
            stroke_type: rect.stroke_type.as_deref(),
            end_cap: rect.end_cap.as_deref(),
            end_join: rect.end_join.as_deref(),
            miter_limit: rect.miter_limit,
            stroke_gap_color: rect.stroke_gap_color.as_deref(),
            stroke_gap_tint: rect.stroke_gap_tint,
            stroke_dash: &rect.stroke_dash,
            corner_radius: rect.corner_radius,
            corner_option: rect.corner_option.as_deref(),
            corners: rect.corners,
            applied_object_style: rect.applied_object_style.as_deref(),
            overprint_fill: rect.overprint_fill,
            overprint_stroke: rect.overprint_stroke,
            geometry,
        }
    }

    pub(crate) fn from_oval(oval: &'a Oval) -> Self {
        Self {
            self_id: oval.self_id.as_deref(),
            item_transform: oval.item_transform,
            fill_color: oval.fill_color.as_deref(),
            stroke_color: oval.stroke_color.as_deref(),
            stroke_weight: oval.stroke_weight,
            fill_tint: oval.fill_tint,
            opacity: oval.opacity,
            blend_mode: crate::pipeline::blend_mode_from_idml(oval.blend_mode.as_deref()),
            gradient_fill_angle: oval.gradient_fill_angle,
            gradient_fill_length: oval.gradient_fill_length,
            gradient_stroke_angle: oval.gradient_stroke_angle,
            gradient_stroke_length: oval.gradient_stroke_length,
            drop_shadow: oval.drop_shadow.as_ref(),
            stroke_alignment: None,
            stroke_type: oval.stroke_type.as_deref(),
            end_cap: None,
            end_join: None,
            miter_limit: None,
            stroke_gap_color: oval.stroke_gap_color.as_deref(),
            stroke_gap_tint: oval.stroke_gap_tint,
            stroke_dash: &oval.stroke_dash,
            corner_radius: None,
            corner_option: None,
            corners: Default::default(),
            applied_object_style: oval.applied_object_style.as_deref(),
            overprint_fill: oval.overprint_fill,
            overprint_stroke: oval.overprint_stroke,
            geometry: Geometry::Oval {
                rect: rect_from_bounds(oval.bounds),
            },
        }
    }

    pub(crate) fn from_polygon(poly: &'a Polygon) -> Self {
        let bbox = rect_from_bounds(poly.bounds);
        // Synthetic IDMLs sometimes omit anchor data; fall back to
        // bbox-as-rect so paint modules never see an empty polygon.
        let geometry = if poly.anchors.is_empty() {
            Geometry::Rect { rect: bbox }
        } else {
            Geometry::Polygon {
                anchors: &poly.anchors,
                subpath_starts: &poly.subpath_starts,
                subpath_open: &poly.subpath_open,
                bbox,
            }
        };
        Self {
            self_id: poly.self_id.as_deref(),
            item_transform: poly.item_transform,
            fill_color: poly.fill_color.as_deref(),
            stroke_color: poly.stroke_color.as_deref(),
            stroke_weight: poly.stroke_weight,
            fill_tint: poly.fill_tint,
            opacity: poly.opacity,
            blend_mode: crate::pipeline::blend_mode_from_idml(poly.blend_mode.as_deref()),
            gradient_fill_angle: poly.gradient_fill_angle,
            gradient_fill_length: poly.gradient_fill_length,
            gradient_stroke_angle: poly.gradient_stroke_angle,
            gradient_stroke_length: poly.gradient_stroke_length,
            drop_shadow: None,
            stroke_alignment: None,
            stroke_type: poly.stroke_type.as_deref(),
            end_cap: None,
            end_join: None,
            miter_limit: None,
            stroke_gap_color: poly.stroke_gap_color.as_deref(),
            stroke_gap_tint: poly.stroke_gap_tint,
            stroke_dash: &poly.stroke_dash,
            corner_radius: None,
            corner_option: None,
            corners: Default::default(),
            applied_object_style: poly.applied_object_style.as_deref(),
            overprint_fill: poly.overprint_fill,
            overprint_stroke: poly.overprint_stroke,
            geometry,
        }
    }

    pub(crate) fn from_graphic_line(line: &'a GraphicLine) -> Self {
        // Lines emit through `transform_bounds` in the legacy path,
        // which maps inner-coord bounds to spread coords, then the
        // page subtracts spread_origin. Flatten the endpoints into
        // page-local coords here so the geometry adapter can render
        // them directly. Done by the caller's `from_graphic_line_in`
        // since we need access to the page's spread_origin — see
        // [`Self::from_graphic_line_in`]. This bare constructor is
        // kept so callers without page context can still flatten;
        // the line endpoints land in inner coords.
        let bounds = line.bounds;
        Self {
            self_id: line.self_id.as_deref(),
            item_transform: line.item_transform,
            fill_color: None,
            stroke_color: line.stroke_color.as_deref(),
            stroke_weight: line.stroke_weight,
            fill_tint: None,
            opacity: None,
            blend_mode: BlendMode::Normal,
            gradient_fill_angle: None,
            gradient_fill_length: None,
            gradient_stroke_angle: None,
            gradient_stroke_length: None,
            drop_shadow: None,
            stroke_alignment: None,
            stroke_type: line.stroke_type.as_deref(),
            end_cap: None,
            end_join: None,
            miter_limit: None,
            stroke_gap_color: line.stroke_gap_color.as_deref(),
            stroke_gap_tint: line.stroke_gap_tint,
            stroke_dash: &line.stroke_dash,
            corner_radius: None,
            corner_option: None,
            corners: Default::default(),
            applied_object_style: line.applied_object_style.as_deref(),
            overprint_fill: false,
            overprint_stroke: line.overprint_stroke,
            geometry: Geometry::Line {
                p0: (bounds.left, bounds.top),
                p1: (bounds.right, bounds.bottom),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_parse::{Bounds, PathAnchor};

    fn rect_with_anchors(anchors: Vec<PathAnchor>) -> Rectangle {
        Rectangle {
            self_id: None,
            bounds: Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 10.0,
                right: 10.0,
            },
            item_transform: None,
            fill_color: None,
            fill_tint: None,
            stroke_color: None,
            stroke_weight: None,
            drop_shadow: None,
            stroke_drop_shadow: None,
            image_link: None,
            has_image_element: false,
            has_inline_pdf: false,
            image_item_transform: None,
            image_bytes: None,
            applied_object_style: None,
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
            is_anchored: false,
            opacity: None,
            blend_mode: None,
            effects: None,
            gradient_fill_angle: None,
            gradient_fill_length: None,
            gradient_stroke_angle: None,
            gradient_stroke_length: None,
            text_paths: Vec::new(),
            overprint_fill: false,
            overprint_stroke: false,
            nonprinting: false,
            anchors,
            subpath_starts: Vec::new(),
            subpath_open: Vec::new(),
        }
    }

    fn pa(x: f32, y: f32) -> PathAnchor {
        PathAnchor {
            anchor: (x, y),
            left: (x, y),
            right: (x, y),
        }
    }

    fn text_frame_with(anchors: Vec<PathAnchor>) -> TextFrame {
        TextFrame {
            self_id: None,
            parent_story: None,
            bounds: Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 20.0,
                right: 20.0,
            },
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
            applied_object_style: None,
            text_wrap: None,
            item_layer: None,
            is_anchored: false,
            opacity: None,
            blend_mode: None,
            anchors,
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
        }
    }

    #[test]
    fn q11_rectangle_with_four_or_fewer_anchors_stays_rect_geometry() {
        // 4 anchors (AABB) → keep Rect path. Empty anchors → same.
        let r0 = rect_with_anchors(Vec::new());
        let r4 = rect_with_anchors(vec![
            pa(0.0, 0.0),
            pa(10.0, 0.0),
            pa(10.0, 10.0),
            pa(0.0, 10.0),
        ]);
        for r in [r0, r4] {
            let frame = ResolvedFrame::from_rectangle(&r);
            assert!(
                matches!(frame.geometry, Geometry::Rect { .. }),
                "≤4 anchors must keep Rect geometry"
            );
        }
    }

    #[test]
    fn q11_rectangle_with_many_anchors_routes_to_polygon_geometry() {
        // 8-anchor stylised outline — the Q-11 case.
        let anchors: Vec<PathAnchor> = (0..8)
            .map(|i| {
                let t = i as f32;
                pa(t, t.sin() * 3.0 + 5.0)
            })
            .collect();
        let r = rect_with_anchors(anchors);
        let frame = ResolvedFrame::from_rectangle(&r);
        match frame.geometry {
            Geometry::Polygon { anchors, bbox, .. } => {
                assert_eq!(anchors.len(), 8, "all anchors threaded through");
                assert_eq!(bbox.x, 0.0);
                assert_eq!(bbox.w, 10.0);
            }
            _ => panic!("multi-anchor Rectangle must lift to Polygon geometry"),
        }
    }

    /// W1.1: a plain axis-aligned 4-anchor text-frame path (the
    /// serialisation every ordinary text panel carries) and an empty
    /// path both stay on the cheap `TextFrameRect` emitter — they're
    /// "rectangular".
    #[test]
    fn text_frame_rect_path_recognises_plain_rectangle() {
        let rect4 = vec![pa(0.0, 0.0), pa(20.0, 0.0), pa(20.0, 10.0), pa(0.0, 10.0)];
        assert!(text_frame_is_rect_path(&rect4, &[], &[]));
        assert!(text_frame_is_rect_path(&[], &[], &[]), "empty path = rect");
    }

    /// W1.1: a non-rectangular outline (triangle), a compound path, a
    /// curved-sided box, and an explicitly-open path all fail the
    /// rectangularity test, so `from_text_frame` lifts them to a real
    /// `Geometry::Polygon` and the frame paints its actual fill/stroke.
    #[test]
    fn text_frame_rect_path_rejects_non_rectangular_shapes() {
        // Triangle: 3 anchors → not a rect.
        let tri = vec![pa(0.0, 0.0), pa(20.0, 0.0), pa(10.0, 20.0)];
        assert!(!text_frame_is_rect_path(&tri, &[], &[]));

        // Four axis-aligned corners but a compound path (two contours).
        let two_contours = vec![pa(0.0, 0.0), pa(20.0, 0.0), pa(20.0, 10.0), pa(0.0, 10.0)];
        assert!(!text_frame_is_rect_path(&two_contours, &[0, 2], &[]));

        // Four axis-aligned corners but the path is flagged open.
        let open = vec![pa(0.0, 0.0), pa(20.0, 0.0), pa(20.0, 10.0), pa(0.0, 10.0)];
        assert!(!text_frame_is_rect_path(&open, &[], &[true]));

        // Four corners whose top-left anchor carries a non-coincident
        // handle: a curved side, so not a plain rect.
        let curved = vec![
            PathAnchor {
                anchor: (0.0, 0.0),
                left: (0.0, 0.0),
                right: (5.0, -3.0), // handle bows the top edge outward
            },
            pa(20.0, 0.0),
            pa(20.0, 10.0),
            pa(0.0, 10.0),
        ];
        assert!(!text_frame_is_rect_path(&curved, &[], &[]));
    }

    /// W1.1: the lift is wired end-to-end through `from_text_frame` —
    /// a triangular text frame produces `Geometry::Polygon` (so the
    /// fill/stroke modules paint the real outline) while a plain
    /// rectangular one stays `TextFrameRect`.
    #[test]
    fn from_text_frame_lifts_non_rect_path_to_polygon() {
        let tri = text_frame_with(vec![pa(0.0, 0.0), pa(20.0, 0.0), pa(10.0, 20.0)]);
        assert!(
            matches!(
                ResolvedFrame::from_text_frame(&tri).geometry,
                Geometry::Polygon { .. }
            ),
            "triangular text frame must lift to Polygon"
        );
        let rect = text_frame_with(vec![
            pa(0.0, 0.0),
            pa(20.0, 0.0),
            pa(20.0, 10.0),
            pa(0.0, 10.0),
        ]);
        assert!(
            matches!(
                ResolvedFrame::from_text_frame(&rect).geometry,
                Geometry::TextFrameRect { .. }
            ),
            "plain rectangular text frame must stay TextFrameRect"
        );
    }

    #[test]
    fn cycle4_track3_stroke_type_threads_from_oval_into_resolved_frame() {
        use paged_parse::Oval;
        let oval = Oval {
            self_id: None,
            bounds: Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 10.0,
                right: 10.0,
            },
            item_transform: None,
            fill_color: None,
            fill_tint: None,
            stroke_color: None,
            stroke_weight: None,
            stroke_type: Some("StrokeStyle/$ID/Dashed".to_string()),
            stroke_gap_color: None,
            stroke_gap_tint: None,
            stroke_dash: Vec::new(),
            drop_shadow: None,
            stroke_drop_shadow: None,
            applied_object_style: None,
            text_wrap: None,
            item_layer: None,
            effects: None,
            gradient_fill_angle: None,
            gradient_fill_length: None,
            gradient_stroke_angle: None,
            gradient_stroke_length: None,
            opacity: None,
            blend_mode: None,
            image_link: None,
            image_bytes: None,
            has_image_element: false,
            has_inline_pdf: false,
            image_item_transform: None,
            overprint_fill: false,
            overprint_stroke: false,
            nonprinting: false,
        };
        let frame = ResolvedFrame::from_oval(&oval);
        assert_eq!(frame.stroke_type, Some("StrokeStyle/$ID/Dashed"));
    }

    #[test]
    fn cycle4_track3_stroke_type_threads_from_polygon_into_resolved_frame() {
        use paged_parse::Polygon;
        let poly = Polygon {
            self_id: None,
            bounds: Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 10.0,
                right: 10.0,
            },
            item_transform: None,
            fill_color: None,
            fill_tint: None,
            stroke_color: None,
            stroke_weight: None,
            stroke_type: Some("StrokeStyle/$ID/Dotted".to_string()),
            stroke_gap_color: None,
            stroke_gap_tint: None,
            stroke_dash: Vec::new(),
            applied_object_style: None,
            anchors: Vec::new(),
            subpath_starts: Vec::new(),
            subpath_open: Vec::new(),
            text_wrap: None,
            item_layer: None,
            effects: None,
            gradient_fill_angle: None,
            gradient_fill_length: None,
            gradient_stroke_angle: None,
            gradient_stroke_length: None,
            opacity: None,
            blend_mode: None,
            text_paths: Vec::new(),
            image_link: None,
            has_image_element: false,
            has_inline_pdf: false,
            image_item_transform: None,
            image_bytes: None,
            overprint_fill: false,
            overprint_stroke: false,
            nonprinting: false,
        };
        let frame = ResolvedFrame::from_polygon(&poly);
        assert_eq!(frame.stroke_type, Some("StrokeStyle/$ID/Dotted"));
    }

    #[test]
    fn cycle4_track3_stroke_type_threads_from_graphic_line_into_resolved_frame() {
        use paged_parse::GraphicLine;
        let line = GraphicLine {
            self_id: None,
            bounds: Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 10.0,
                right: 10.0,
            },
            item_transform: None,
            stroke_color: None,
            stroke_weight: None,
            stroke_type: Some("CustomDashStyle".to_string()),
            stroke_gap_color: None,
            stroke_gap_tint: None,
            stroke_dash: Vec::new(),
            applied_object_style: None,
            text_wrap: None,
            item_layer: None,
            anchors: Vec::new(),
            subpath_starts: Vec::new(),
            subpath_open: Vec::new(),
            text_paths: Vec::new(),
            effects: None,
            overprint_stroke: false,
            nonprinting: false,
            start_arrow: paged_parse::ArrowheadType::None,
            end_arrow: paged_parse::ArrowheadType::None,
            start_arrow_scale: 100.0,
            end_arrow_scale: 100.0,
        };
        let frame = ResolvedFrame::from_graphic_line(&line);
        assert_eq!(frame.stroke_type, Some("CustomDashStyle"));
    }
}
