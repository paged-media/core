//! Top-level renderer.
//!
//! Public Rust API. Coordinates parse → scene → text layout → compose →
//! GPU raster. Mirrors the TypeScript surface described in idea.md §14.

pub mod asset;
pub mod pipeline;

mod module;

pub use asset::{AssetResolver, BytesResolver};
pub use pipeline::{
    build, build_document, build_run_paint_picker, resolve_fill, resolve_stroke, BuiltDocument,
    BuiltPage, ClusterPos, FontMetricsOverride, FontTable, LineLayout, MasterTextEmitDelta,
    PageId, PipelineOptions, PipelineStats, RunPaintPicker,
};

#[cfg(feature = "cpu")]
pub use pipeline::{render, render_built_page, render_document};

// Re-export Document so consumers only need one `use` for the common
// path: `use idml_renderer::{Document, pipeline, PipelineOptions};`.
pub use idml_scene::Document;

// Re-export the display-list IR so canvas crates depend on a single
// upstream and don't pull `idml-compose` directly.
pub use idml_compose::{DisplayCommand, DisplayList};
