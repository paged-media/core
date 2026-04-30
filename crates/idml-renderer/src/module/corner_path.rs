//! Corner-path module — Rectangle-only.
//!
//! Builds the rounded-corner path for a Rectangle whose
//! `CornerOption` selects one of the rounding variants and returns
//! the interned `(fill_path, stroke_path)` pair so the fill / stroke
//! modules emit `FillPath{Blend}` / `StrokePath` instead of axis-
//! aligned rect primitives. Returns `(None, None)` for non-Rectangle
//! geometries or rectangles without a positive corner radius.

use idml_compose::PathId;

use super::{Geometry, ResolvedFrame};
use crate::pipeline::{
    corner_radius_from, fnv_1a_u64, inset_rect, rounded_rect_path, stroke_alignment_offset,
    BuiltPage,
};

pub(crate) struct CornerPaths {
    pub fill: Option<PathId>,
    pub stroke: Option<PathId>,
}

impl CornerPaths {
    pub fn none() -> Self {
        Self {
            fill: None,
            stroke: None,
        }
    }
}

pub(crate) fn corner_path_module(frame: &ResolvedFrame<'_>, page: &mut BuiltPage) -> CornerPaths {
    let Geometry::Rect { rect } = frame.geometry else {
        return CornerPaths::none();
    };
    let Some(radius) = corner_radius_from(frame.corner_radius, frame.corner_option) else {
        return CornerPaths::none();
    };
    let path = rounded_rect_path(rect, radius);
    let key_bytes = frame
        .self_id
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_else(|| format!("{:?}", rect).into_bytes());
    let fill_key = fnv_1a_u64(&[key_bytes.as_slice(), &radius.to_bits().to_le_bytes()].concat());
    let (fill_id, _) = page.list.paths.intern(fill_key, path);

    // Stroke alignment shifts the stroke path inward (Inside) /
    // outward (Outside) by W/2 with the radius adjusted to keep the
    // corners tangent to the geometry — same math the legacy emit
    // ran inline.
    let stroke_offset = stroke_alignment_offset(frame.stroke_alignment, frame.stroke_weight);
    let stroke_rect = inset_rect(rect, stroke_offset);
    let stroke_radius = (radius - stroke_offset).max(0.0);
    let stroke_path = rounded_rect_path(stroke_rect, stroke_radius);
    let stroke_key = fnv_1a_u64(
        &[
            key_bytes.as_slice(),
            &stroke_radius.to_bits().to_le_bytes(),
            &stroke_offset.to_bits().to_le_bytes(),
            b"sa",
        ]
        .concat(),
    );
    let (stroke_id, _) = page.list.paths.intern(stroke_key, stroke_path);

    CornerPaths {
        fill: Some(fill_id),
        stroke: Some(stroke_id),
    }
}
