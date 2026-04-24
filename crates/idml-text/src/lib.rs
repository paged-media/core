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

pub mod compose;
pub mod layout;
pub mod shape;

pub use compose::{
    compose_paragraph, AdvanceMeasurer, ComposeOptions, ComposedLine, MonospaceMeasurer,
    RustybuzzMeasurer, TextShaper,
};
pub use layout::{
    layout_paragraph, position_line, LaidOutLine, LaidOutParagraph, LayoutOptions, PositionedGlyph,
};
pub use shape::{shape_run, ShapedGlyph, ShapedRun};
