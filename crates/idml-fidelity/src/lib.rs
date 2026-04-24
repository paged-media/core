//! Fidelity corpus harness.
//!
//! - Reference rasterisation (Ghostscript on Linux, CoreGraphics on macOS)
//! - Per-pixel ΔE2000 + SSIM diff with heatmap overlays
//! - Golden-image store and CI gate
//!
//! This crate is built first (before the renderer itself) so every
//! downstream change is measurable from day one.

/// Pass criteria from idea.md §13.2.
pub const MEAN_DELTA_E_THRESHOLD: f64 = 1.0;
pub const P99_DELTA_E_THRESHOLD: f64 = 2.5;
pub const SSIM_THRESHOLD: f64 = 0.99;
pub const MAX_GLYPH_MISPLACEMENT_PT: f64 = 0.5;
