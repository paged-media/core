//! Fidelity corpus harness.
//!
//! - Reference rasterisation (Ghostscript on Linux, CoreGraphics on macOS)
//! - Per-pixel ΔE2000 + SSIM diff with heatmap overlays
//! - Golden-image store and CI gate
//!
//! This crate is built first (before the renderer itself) so every
//! downstream change is measurable from day one.

pub mod color;
pub mod diff;
pub mod ssim;

/// Pass criteria from idea.md §13.2.
pub const MEAN_DELTA_E_THRESHOLD: f64 = 1.0;
pub const P99_DELTA_E_THRESHOLD: f64 = 2.5;
pub const SSIM_THRESHOLD: f64 = 0.99;
pub const MAX_GLYPH_MISPLACEMENT_PT: f64 = 0.5;

/// Aggregate verdict for a single comparison.
#[derive(Debug, Clone)]
pub struct FidelityReport {
    pub mean_delta_e: f64,
    pub p99_delta_e: f64,
    pub max_delta_e: f64,
    pub ssim: f64,
    pub width: u32,
    pub height: u32,
}

impl FidelityReport {
    /// Whether the report meets the idea.md §13.2 pass criteria.
    pub fn passes(&self) -> bool {
        self.mean_delta_e <= MEAN_DELTA_E_THRESHOLD
            && self.p99_delta_e <= P99_DELTA_E_THRESHOLD
            && self.ssim >= SSIM_THRESHOLD
    }
}
