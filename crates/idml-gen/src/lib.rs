//! IDML sample-corpus generator.
//!
//! Produces deterministic IDML packages for the renderer's diff
//! harness. Each emitted `.idml` is a multi-page document whose pages
//! each exercise one renderable feature variant — failure attribution
//! comes from per-page heatmaps + `Page.Name` carrying the variant
//! descriptor, so a single InDesign export covers many test cases.
//!
//! See `docs/idml-sample-generator.md` for the strategic argument and
//! `crates/idml-gen/src/samples/` for the concrete sample definitions.

pub mod geometry;
pub mod ids;
pub mod package;
pub mod xml;

pub mod builders;
pub mod samples;

pub use package::{write_idml, Sample};
