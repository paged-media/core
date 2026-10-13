/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

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
/// A pixel is ink when its CIE L* is below this in either image.
pub const INK_LIGHTNESS: f64 = 60.0;

/// Aggregate verdict for a single comparison.
#[derive(Debug, Clone)]
pub struct FidelityReport {
    pub mean_delta_e: f64,
    pub p99_delta_e: f64,
    pub max_delta_e: f64,
    pub ssim: f64,
    /// Mean ΔE over the pixels either image inks (L* below
    /// [`INK_LIGHTNESS`]): text, rules and dark fills. A page is mostly
    /// paper, so text set a few points off moves the page-wide mean by
    /// almost nothing (type on a path drawn 165 pt from where InDesign set
    /// it read a mean of 0.136); over its ink it reads as what it is.
    /// `0.0` on a page with no ink.
    pub ink_mean_delta_e: f64,
    /// How many pixels count as ink in either image.
    pub ink_pixels: u64,
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
