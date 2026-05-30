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

//! Corner-path module — Rectangle-only.
//!
//! Builds the rounded-corner path for a Rectangle whose
//! `CornerOption` selects one of the rounding variants and returns
//! the interned `(fill_path, stroke_path)` pair so the fill / stroke
//! modules emit `FillPath{Blend}` / `StrokePath` instead of axis-
//! aligned rect primitives. Returns `(None, None)` for non-Rectangle
//! geometries or rectangles without a positive corner radius.

use paged_compose::PathId;

use super::{Geometry, ResolvedFrame};
use crate::pipeline::{
    fnv_1a_u64, inset_rect, per_corner_radii, rounded_rect_path_per_corner,
    stroke_alignment_offset, BuiltPage,
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
    // Q-16: resolve 4 per-corner radii (falls back to the symmetric
    // `corner_radius` / `corner_option` pair when no per-corner
    // override is set).
    let radii = per_corner_radii(frame.corner_radius, frame.corner_option, &frame.corners);
    if radii.iter().all(|r| r.is_none()) {
        return CornerPaths::none();
    }
    let path = rounded_rect_path_per_corner(rect, radii);
    let key_bytes = frame
        .self_id
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_else(|| format!("{:?}", rect).into_bytes());
    let mut radii_bits = [0u8; 16];
    for (i, r) in radii.iter().enumerate() {
        let v = r.unwrap_or(0.0).to_bits().to_le_bytes();
        radii_bits[i * 4..i * 4 + 4].copy_from_slice(&v);
    }
    let fill_key = fnv_1a_u64(&[key_bytes.as_slice(), &radii_bits].concat());
    let (fill_id, _) = page.list.paths.intern(fill_key, path);

    // Stroke alignment shifts the stroke path inward (Inside) /
    // outward (Outside) by W/2 with each radius adjusted to keep the
    // corners tangent to the geometry — same math the legacy emit
    // ran inline, applied per corner now.
    let stroke_offset =
        stroke_alignment_offset(frame.stroke_alignment, frame.effective_stroke_weight());
    let stroke_rect = inset_rect(rect, stroke_offset);
    let stroke_radii: [Option<f32>; 4] = [
        radii[0].map(|r| (r - stroke_offset).max(0.0)),
        radii[1].map(|r| (r - stroke_offset).max(0.0)),
        radii[2].map(|r| (r - stroke_offset).max(0.0)),
        radii[3].map(|r| (r - stroke_offset).max(0.0)),
    ];
    let stroke_path = rounded_rect_path_per_corner(stroke_rect, stroke_radii);
    let mut stroke_bits = [0u8; 16];
    for (i, r) in stroke_radii.iter().enumerate() {
        let v = r.unwrap_or(0.0).to_bits().to_le_bytes();
        stroke_bits[i * 4..i * 4 + 4].copy_from_slice(&v);
    }
    let stroke_key = fnv_1a_u64(
        &[
            key_bytes.as_slice(),
            &stroke_bits,
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
