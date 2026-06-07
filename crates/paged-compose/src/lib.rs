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

//! Display-list compositor.
//!
//! Walks the laid-out scene graph and emits a structured command
//! buffer: paths, fills, clips, blend state, effects. The display
//! list is the handoff format to the GPU rasterizer and is versioned
//! so it can also be used as a stable intermediate representation for
//! tooling.

pub mod display_list;
pub mod glyph;
pub mod primitives;
pub mod text;

pub use display_list::{
    BevelEmboss, BlendMode, Color, DashPattern, DecodedImage, DirectionalFeather, DisplayCommand,
    DisplayList, DropShadow, Feather, FeatherCornerType, GlyphCacheKey, GlyphRunEntry,
    GlyphRunTable, GradientFeather, GradientFeatherKind, GradientFeatherStop, GradientId,
    GradientStop, ImageId, InnerGlow, InnerShadow, LayerEffect, LineCap, LineJoin, LinearGradient,
    LinkRegion, LinkRegionTable, LinkTarget, OuterGlow, Paint, PathBuffer, PathData, PathId,
    PathSegment, RadialGradient, Rect, Satin, SpotInk, SpotInkId, Stroke, Transform,
};
pub use glyph::{GlyphOutliner, TtfOutliner, UnitSquareOutliner};
pub use primitives::{
    emit_drop_shadow_rect, emit_drop_shadow_rect_transformed, emit_ellipse,
    emit_ellipse_transformed, emit_ellipse_transformed_blend, emit_image_at, emit_line, emit_rect,
    emit_rect_transformed, emit_rect_transformed_blend, emit_stroke_ellipse,
    emit_stroke_ellipse_transformed, emit_stroke_rect, emit_stroke_rect_transformed, unit_ellipse,
    UNIT_ELLIPSE_KEY, UNIT_RECT_KEY,
};
pub use text::{
    emit_glyph_slice, emit_glyph_slice_blend, emit_glyph_slice_stroke, emit_paragraph,
    emit_paragraph_blend,
};
