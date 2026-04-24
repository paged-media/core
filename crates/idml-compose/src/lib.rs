//! Display-list compositor.
//!
//! Walks the laid-out scene graph and emits a structured command
//! buffer: paths, fills, clips, blend state, effects. The display
//! list is the handoff format to the GPU rasterizer and is versioned
//! so it can also be used as a stable intermediate representation for
//! tooling.

pub mod display_list;
pub mod glyph;
pub mod text;

pub use display_list::{
    Color, DisplayCommand, DisplayList, GlyphCacheKey, Paint, PathBuffer, PathData, PathId,
    PathSegment, Rect, Transform,
};
pub use glyph::{GlyphOutliner, TtfOutliner, UnitSquareOutliner};
pub use text::emit_paragraph;
