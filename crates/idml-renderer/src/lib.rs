//! Top-level renderer.
//!
//! Public Rust API. Coordinates parse → scene → text layout → compose →
//! GPU raster. Mirrors the TypeScript surface described in idea.md §14.

pub mod pipeline;

pub use pipeline::{
    build, build_run_paint_picker, derive_story_id, resolve_fill, resolve_stroke, BuiltPage,
    PipelineOptions, PipelineStats, RunPaintPicker,
};

#[cfg(feature = "cpu")]
pub use pipeline::render;
