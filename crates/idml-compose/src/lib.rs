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
    Color, DisplayCommand, DisplayList, DropShadow, GlyphCacheKey, GradientId, GradientStop,
    LineCap, LineJoin, LinearGradient, Paint, PathBuffer, PathData, PathId, PathSegment, Rect,
    Stroke, Transform,
};
pub use glyph::{GlyphOutliner, TtfOutliner, UnitSquareOutliner};
pub use primitives::{
    emit_drop_shadow_rect, emit_ellipse, emit_line, emit_rect, emit_stroke_ellipse,
    emit_stroke_rect, UNIT_ELLIPSE_KEY, UNIT_RECT_KEY,
};
pub use text::emit_paragraph;
