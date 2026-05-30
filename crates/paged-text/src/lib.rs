//! Text engine.
//!
//! Highest-risk subsystem; roughly 40% of project effort. Responsibilities:
//! - shaping each homogeneous run via rustybuzz
//! - Knuth-Plass line breaking with InDesign-calibrated penalty weights
//! - hyphenation (TeX patterns by default; Proximity if licensed)
//! - composition into frame-bound layouts with justification
//!
//! Calibration of Paragraph Composer parity happens in
//! `spikes/composer-calibration` before this crate takes a hard dependency
//! on any specific penalty configuration.

pub mod cache;
pub mod compose;
pub mod hyphenate;
pub mod layout;
pub mod shape;

pub use cache::{CacheStats, LayoutCache, LayoutKeyHasher};
pub use compose::{
    compose_paragraph, compose_paragraph_with_drop_cap, drop_cap_column_widths,
    drop_cap_column_widths_with_min, drop_cap_point_size, AdvanceMeasurer, ComposeOptions,
    ComposedLine, DropCapComposition,
    DropCapSpec, MonospaceMeasurer, RustybuzzMeasurer, TextShaper,
};
pub use hyphenate::{Hyphenator, Language};
pub use layout::{
    apply_bidi_reorder, layout_paragraph, layout_runs, position_line, Alignment, BidiDirection,
    LaidOutLine, LaidOutParagraph, LayoutOptions, PositionedGlyph, StyledRun,
};
pub use shape::{
    apply_optical_margin, apply_tracking, optical_margin_offset, shape_run,
    shape_run_with_features, KerningMethod, MarginSide, ShapedGlyph, ShapedRun, ShapingFeatures,
};
