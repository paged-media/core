//! Top-level renderer.
//!
//! Public Rust API. Coordinates parse → scene → text layout → compose →
//! GPU raster. Mirrors the TypeScript surface described in idea.md §14.

pub mod asset;
pub mod pipeline;

pub use asset::{AssetResolver, BytesResolver};
pub use pipeline::{
    build, build_document, build_run_paint_picker, resolve_fill, resolve_stroke, BuiltDocument,
    BuiltPage, PipelineOptions, PipelineStats, RunPaintPicker,
};

#[cfg(feature = "cpu")]
pub use pipeline::{render, render_document};

// Re-export Document so consumers only need one `use` for the common
// path: `use idml_renderer::{Document, pipeline, PipelineOptions};`.
pub use idml_scene::Document;
