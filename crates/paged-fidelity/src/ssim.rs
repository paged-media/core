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

//! Structural similarity index (luminance-only Wang 2004).
//!
//! We want a single-scalar structural-similarity number for the whole
//! frame. The CI gate only needs a threshold check (≥ 0.99), so we
//! implement the luminance-channel mean-window variant: partition the
//! image into 8×8 blocks, compute per-block SSIM, and average.

use image::RgbImage;

const K1: f64 = 0.01;
const K2: f64 = 0.03;
const L: f64 = 255.0;
const C1: f64 = (K1 * L) * (K1 * L);
const C2: f64 = (K2 * L) * (K2 * L);
const BLOCK: u32 = 8;

/// Compute mean SSIM between two RGB images of identical dimensions.
pub fn ssim(a: &RgbImage, b: &RgbImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions(), "size mismatch");
    let (w, h) = a.dimensions();

    // Precompute luminance channel once per image.
    let mut la = vec![0f64; (w * h) as usize];
    let mut lb = vec![0f64; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let pa = a.get_pixel(x, y);
            let pb = b.get_pixel(x, y);
            la[(y * w + x) as usize] = luma(pa.0);
            lb[(y * w + x) as usize] = luma(pb.0);
        }
    }

    let mut total = 0.0;
    let mut n = 0u64;
    let mut y = 0;
    while y + BLOCK <= h {
        let mut x = 0;
        while x + BLOCK <= w {
            total += block_ssim(&la, &lb, w, x, y);
            n += 1;
            x += BLOCK;
        }
        y += BLOCK;
    }
    if n == 0 {
        1.0
    } else {
        total / n as f64
    }
}

fn luma(rgb: [u8; 3]) -> f64 {
    // Rec. 601 luma; adequate for structural comparison.
    0.299 * rgb[0] as f64 + 0.587 * rgb[1] as f64 + 0.114 * rgb[2] as f64
}

fn block_ssim(a: &[f64], b: &[f64], w: u32, x0: u32, y0: u32) -> f64 {
    let n = (BLOCK * BLOCK) as f64;
    let mut sa = 0.0;
    let mut sb = 0.0;
    for dy in 0..BLOCK {
        for dx in 0..BLOCK {
            let i = ((y0 + dy) * w + (x0 + dx)) as usize;
            sa += a[i];
            sb += b[i];
        }
    }
    let ma = sa / n;
    let mb = sb / n;

    let mut va = 0.0;
    let mut vb = 0.0;
    let mut cov = 0.0;
    for dy in 0..BLOCK {
        for dx in 0..BLOCK {
            let i = ((y0 + dy) * w + (x0 + dx)) as usize;
            let da = a[i] - ma;
            let db = b[i] - mb;
            va += da * da;
            vb += db * db;
            cov += da * db;
        }
    }
    va /= n;
    vb /= n;
    cov /= n;

    let num = (2.0 * ma * mb + C1) * (2.0 * cov + C2);
    let den = (ma * ma + mb * mb + C1) * (va + vb + C2);
    num / den
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn identical_images_ssim_one() {
        let img = ImageBuffer::from_fn(64, 64, |x, y| Rgb([((x + y) & 0xff) as u8, 128, 200]));
        let s = ssim(&img, &img);
        assert!((s - 1.0).abs() < 1e-9, "got {}", s);
    }

    #[test]
    fn one_pixel_shift_drops_ssim_below_one() {
        let a = ImageBuffer::from_fn(64, 64, |x, y| Rgb([((x + y) & 0xff) as u8, 128, 200]));
        let mut b = a.clone();
        b.put_pixel(32, 32, Rgb([0, 0, 0]));
        let s = ssim(&a, &b);
        assert!(s < 1.0, "expected < 1.0, got {}", s);
        assert!(s > 0.9, "degradation should be localised, got {}", s);
    }
}
