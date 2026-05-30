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

//! Per-pixel ΔE2000 diffing with optional heatmap output.

use std::path::Path;

use anyhow::{Context, Result};
use image::{ImageBuffer, Rgb, RgbImage};

use crate::{color, FidelityReport};

/// Compare two PNGs and compute a fidelity report.
pub fn compare_pngs(reference: &Path, candidate: &Path) -> Result<(FidelityReport, Vec<f64>)> {
    let ref_img = image::open(reference)
        .with_context(|| format!("open {}", reference.display()))?
        .to_rgb8();
    let cand_img = image::open(candidate)
        .with_context(|| format!("open {}", candidate.display()))?
        .to_rgb8();
    compare_images(&ref_img, &cand_img)
}

/// Compare two in-memory RGB images.
///
/// Returns the aggregate report and a per-pixel ΔE field (row-major),
/// which `heatmap` can render.
pub fn compare_images(
    reference: &RgbImage,
    candidate: &RgbImage,
) -> Result<(FidelityReport, Vec<f64>)> {
    anyhow::ensure!(
        reference.dimensions() == candidate.dimensions(),
        "dimension mismatch: reference {:?} vs candidate {:?}",
        reference.dimensions(),
        candidate.dimensions(),
    );
    let (w, h) = reference.dimensions();
    let n = (w * h) as usize;
    let mut deltas = Vec::with_capacity(n);

    let mut sum = 0.0f64;
    let mut max = 0.0f64;
    for (pr, pc) in reference.pixels().zip(candidate.pixels()) {
        let lab_r = color::srgb_u8_to_lab(pr.0[0], pr.0[1], pr.0[2]);
        let lab_c = color::srgb_u8_to_lab(pc.0[0], pc.0[1], pc.0[2]);
        let d = color::delta_e_2000(lab_r, lab_c);
        sum += d;
        if d > max {
            max = d;
        }
        deltas.push(d);
    }
    let mean = sum / n as f64;
    let p99 = percentile(&mut deltas.clone(), 0.99);
    let ssim = crate::ssim::ssim(reference, candidate);

    Ok((
        FidelityReport {
            mean_delta_e: mean,
            p99_delta_e: p99,
            max_delta_e: max,
            ssim,
            width: w,
            height: h,
        },
        deltas,
    ))
}

fn percentile(xs: &mut [f64], p: f64) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((xs.len() - 1) as f64 * p).round() as usize;
    xs[idx]
}

/// Render a ΔE heatmap as a PNG: black = 0, red = `max_scale`, clamped.
pub fn heatmap(w: u32, h: u32, deltas: &[f64], max_scale: f64, out: &Path) -> Result<()> {
    assert_eq!(deltas.len(), (w * h) as usize);
    let mut img: RgbImage = ImageBuffer::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let d = deltas[(y * w + x) as usize];
            let t = (d / max_scale).clamp(0.0, 1.0);
            // Simple black-to-red-to-yellow-to-white ramp.
            let r = (t * 255.0 * 1.5).clamp(0.0, 255.0) as u8;
            let g = ((t - 0.5) * 2.0 * 255.0).clamp(0.0, 255.0) as u8;
            let b = ((t - 0.75) * 4.0 * 255.0).clamp(0.0, 255.0) as u8;
            img.put_pixel(x, y, Rgb([r, g, b]));
        }
    }
    img.save(out)
        .with_context(|| format!("write {}", out.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn identical_images_score_zero_delta_e() {
        let img: RgbImage = ImageBuffer::from_fn(32, 32, |x, _| Rgb([(x * 8) as u8, 0, 0]));
        let (report, deltas) = compare_images(&img, &img).unwrap();
        assert!(report.mean_delta_e < 1e-9);
        assert!(report.max_delta_e < 1e-9);
        assert!(deltas.iter().all(|d| *d < 1e-9));
        assert!(report.passes());
    }

    #[test]
    fn one_pixel_shift_is_detected() {
        let a: RgbImage = ImageBuffer::from_fn(16, 16, |_, _| Rgb([128, 128, 128]));
        let mut b = a.clone();
        b.put_pixel(8, 8, Rgb([255, 0, 0]));
        let (report, _) = compare_images(&a, &b).unwrap();
        assert!(report.max_delta_e > 10.0, "got {}", report.max_delta_e);
        assert!(report.mean_delta_e > 0.0);
    }

    #[test]
    fn fails_on_dimension_mismatch() {
        let a: RgbImage = ImageBuffer::new(16, 16);
        let b: RgbImage = ImageBuffer::new(32, 32);
        assert!(compare_images(&a, &b).is_err());
    }
}
