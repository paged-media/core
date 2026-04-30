//! Flattened renderer IR for page items.
//!
//! Every page-item kind (`TextFrame`, `Rectangle`, `Oval`, `Polygon`,
//! `GraphicLine`) lands in [`ResolvedFrame`] before emission. Modules
//! never name the parser shape types — they read fields off the
//! resolved view and emit through the geometry adapter.

use idml_compose::{BlendMode, Paint, PathId, Rect, Transform};
use idml_parse::{
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
    pub drop_shadow: Option<&'a DropShadowSetting>,
    pub stroke_alignment: Option<&'a str>,
    pub stroke_type: Option<&'a str>,
    pub end_cap: Option<&'a str>,
    pub end_join: Option<&'a str>,
    pub miter_limit: Option<f32>,
    pub corner_radius: Option<f32>,
    pub corner_option: Option<&'a str>,
    pub applied_object_style: Option<&'a str>,
    pub geometry: Geometry<'a>,
}

/// Per-shape geometry. Modules that need to emit a primitive consult
/// this enum through [`super::geometry`] adapters; modules that only
/// read cross-cutting state ignore it.
#[allow(dead_code)]
pub(crate) enum Geometry<'a> {
    Rect { rect: Rect },
    Oval { rect: Rect },
    Line { p0: (f32, f32), p1: (f32, f32) },
    Polygon { anchors: &'a [PathAnchor], bbox: Rect },
    /// TextFrames render as rectangles today; carrying a distinct
    /// variant lets the geometry adapter add path-shaped clipping
    /// later without touching modules.
    TextFrameRect { rect: Rect },
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
    pub palette: &'a idml_parse::Graphic,
    pub cmyk_xform: Option<&'a idml_color::IccTransform>,
    pub fallback_paint: Paint,
    pub fallback_drop_shadow: Option<idml_compose::DropShadow>,
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

fn rect_from_bounds(b: idml_parse::Bounds) -> Rect {
    Rect {
        x: b.left,
        y: b.top,
        w: b.width(),
        h: b.height(),
    }
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
        Self {
            self_id: frame.self_id.as_deref(),
            item_transform: frame.item_transform,
            fill_color: frame.fill_color.as_deref(),
            stroke_color: frame.stroke_color.as_deref(),
            stroke_weight: frame.stroke_weight,
            fill_tint: None,
            opacity: frame.opacity,
            blend_mode: crate::pipeline::blend_mode_from_idml(frame.blend_mode.as_deref()),
            gradient_fill_angle: None,
            drop_shadow: frame.drop_shadow.as_ref(),
            stroke_alignment: None,
            stroke_type: None,
            end_cap: None,
            end_join: None,
            miter_limit: None,
            corner_radius: None,
            corner_option: None,
            applied_object_style: frame.applied_object_style.as_deref(),
            geometry: Geometry::TextFrameRect {
                rect: rect_from_bounds(frame.bounds),
            },
        }
    }

    pub(crate) fn from_rectangle(rect: &'a Rectangle) -> Self {
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
            drop_shadow: rect.drop_shadow.as_ref(),
            stroke_alignment: rect.stroke_alignment.as_deref(),
            stroke_type: rect.stroke_type.as_deref(),
            end_cap: rect.end_cap.as_deref(),
            end_join: rect.end_join.as_deref(),
            miter_limit: rect.miter_limit,
            corner_radius: rect.corner_radius,
            corner_option: rect.corner_option.as_deref(),
            applied_object_style: rect.applied_object_style.as_deref(),
            geometry: Geometry::Rect {
                rect: rect_from_bounds(rect.bounds),
            },
        }
    }

    pub(crate) fn from_oval(oval: &'a Oval) -> Self {
        Self {
            self_id: oval.self_id.as_deref(),
            item_transform: oval.item_transform,
            fill_color: oval.fill_color.as_deref(),
            stroke_color: oval.stroke_color.as_deref(),
            stroke_weight: oval.stroke_weight,
            fill_tint: None,
            opacity: oval.opacity,
            blend_mode: crate::pipeline::blend_mode_from_idml(oval.blend_mode.as_deref()),
            gradient_fill_angle: oval.gradient_fill_angle,
            drop_shadow: oval.drop_shadow.as_ref(),
            stroke_alignment: None,
            stroke_type: None,
            end_cap: None,
            end_join: None,
            miter_limit: None,
            corner_radius: None,
            corner_option: None,
            applied_object_style: oval.applied_object_style.as_deref(),
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
                bbox,
            }
        };
        Self {
            self_id: poly.self_id.as_deref(),
            item_transform: poly.item_transform,
            fill_color: poly.fill_color.as_deref(),
            stroke_color: poly.stroke_color.as_deref(),
            stroke_weight: poly.stroke_weight,
            fill_tint: None,
            opacity: poly.opacity,
            blend_mode: crate::pipeline::blend_mode_from_idml(poly.blend_mode.as_deref()),
            gradient_fill_angle: poly.gradient_fill_angle,
            drop_shadow: None,
            stroke_alignment: None,
            stroke_type: None,
            end_cap: None,
            end_join: None,
            miter_limit: None,
            corner_radius: None,
            corner_option: None,
            applied_object_style: poly.applied_object_style.as_deref(),
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
            drop_shadow: None,
            stroke_alignment: None,
            stroke_type: None,
            end_cap: None,
            end_join: None,
            miter_limit: None,
            corner_radius: None,
            corner_option: None,
            applied_object_style: line.applied_object_style.as_deref(),
            geometry: Geometry::Line {
                p0: (bounds.left, bounds.top),
                p1: (bounds.right, bounds.bottom),
            },
        }
    }
}
