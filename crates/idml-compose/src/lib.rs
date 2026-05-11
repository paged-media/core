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
    DisplayList, DropShadow, Feather, FeatherCornerType, GlyphCacheKey, GradientFeather,
    GradientFeatherKind, GradientFeatherStop, GradientId, GradientStop, ImageId, InnerGlow,
    InnerShadow, LayerEffect, LineCap, LineJoin, LinearGradient, OuterGlow, Paint, PathBuffer,
    PathData, PathId, PathSegment, RadialGradient, Rect, Satin, SpotInk, SpotInkId, Stroke,
    Transform,
};
pub use glyph::{GlyphOutliner, TtfOutliner, UnitSquareOutliner};
pub use primitives::{
    emit_drop_shadow_rect, emit_drop_shadow_rect_transformed, emit_ellipse,
    emit_ellipse_transformed, emit_ellipse_transformed_blend, emit_image_at, emit_line,
    emit_rect, emit_rect_transformed, emit_rect_transformed_blend, emit_stroke_ellipse,
    emit_stroke_ellipse_transformed, emit_stroke_rect, emit_stroke_rect_transformed,
    UNIT_ELLIPSE_KEY, UNIT_RECT_KEY,
};
pub use text::{emit_glyph_slice, emit_glyph_slice_blend, emit_paragraph, emit_paragraph_blend};
