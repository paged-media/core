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

//! sRGB ↔ CIE Lab (D65) conversions for fidelity diffing.
//!
//! Kept self-contained so the harness has zero runtime dependencies on
//! `paged-color`. Accuracy is sufficient for ΔE2000 scoring.

/// Linearise a gamma-encoded sRGB channel (0..=1).
#[inline]
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB (0..=1) → CIE XYZ (D65).
pub fn srgb_to_xyz(r: f64, g: f64, b: f64) -> [f64; 3] {
    let rl = srgb_to_linear(r);
    let gl = srgb_to_linear(g);
    let bl = srgb_to_linear(b);
    let x = 0.412_456_4 * rl + 0.357_576_1 * gl + 0.180_437_5 * bl;
    let y = 0.212_672_9 * rl + 0.715_152_2 * gl + 0.072_175_0 * bl;
    let z = 0.019_333_9 * rl + 0.119_192_0 * gl + 0.950_304_1 * bl;
    [x, y, z]
}

/// CIE XYZ (D65) → CIE Lab.
pub fn xyz_to_lab(x: f64, y: f64, z: f64) -> [f64; 3] {
    // D65 reference white.
    const XN: f64 = 0.950_47;
    const YN: f64 = 1.000_00;
    const ZN: f64 = 1.088_83;

    fn f(t: f64) -> f64 {
        const DELTA: f64 = 6.0 / 29.0;
        if t > DELTA * DELTA * DELTA {
            t.cbrt()
        } else {
            t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
        }
    }

    let fx = f(x / XN);
    let fy = f(y / YN);
    let fz = f(z / ZN);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// sRGB byte triplet (0..=255) → CIE Lab.
pub fn srgb_u8_to_lab(r: u8, g: u8, b: u8) -> [f64; 3] {
    let [x, y, z] = srgb_to_xyz(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    xyz_to_lab(x, y, z)
}

/// CIE ΔE2000 between two Lab colours.
///
/// Reference: Sharma, Wu, Dalal (2005). Implementation follows the
/// canonical formulation; constants are standard.
pub fn delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;

    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let c_mean = (c1 + c2) / 2.0;

    let g = 0.5 * (1.0 - (c_mean.powi(7) / (c_mean.powi(7) + 25f64.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;

    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();

    fn hp(a: f64, b: f64) -> f64 {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            let h = b.atan2(a).to_degrees();
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        }
    }
    let h1p = hp(a1p, b1);
    let h2p = hp(a2p, b2);

    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dh_prime = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();

    let lp_mean = (l1 + l2) / 2.0;
    let cp_mean = (c1p + c2p) / 2.0;
    let hp_mean = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };

    let t = 1.0 - 0.17 * ((hp_mean - 30.0).to_radians()).cos()
        + 0.24 * ((2.0 * hp_mean).to_radians()).cos()
        + 0.32 * ((3.0 * hp_mean + 6.0).to_radians()).cos()
        - 0.20 * ((4.0 * hp_mean - 63.0).to_radians()).cos();

    let d_theta = 30.0 * (-(((hp_mean - 275.0) / 25.0).powi(2))).exp();
    let rc = 2.0 * (cp_mean.powi(7) / (cp_mean.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + (0.015 * (lp_mean - 50.0).powi(2)) / (20.0 + (lp_mean - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cp_mean;
    let sh = 1.0 + 0.015 * cp_mean * t;
    let rt = -((2.0 * d_theta).to_radians()).sin() * rc;

    ((dlp / sl).powi(2)
        + (dcp / sc).powi(2)
        + (dh_prime / sh).powi(2)
        + rt * (dcp / sc) * (dh_prime / sh))
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_colours_are_zero() {
        let lab = srgb_u8_to_lab(128, 64, 200);
        assert!(delta_e_2000(lab, lab) < 1e-9);
    }

    #[test]
    fn known_reference_pair() {
        // Sharma et al. 2005, table of reference values.
        // Lab1=(50.0, 2.6772, -79.7751), Lab2=(50.0, 0.0, -82.7485) → ΔE00 ≈ 2.0425
        let d = delta_e_2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
        assert!((d - 2.0425).abs() < 0.01, "got {}", d);
    }

    #[test]
    fn single_pixel_shift_is_above_threshold() {
        let a = srgb_u8_to_lab(200, 100, 50);
        let b = srgb_u8_to_lab(210, 100, 50);
        assert!(delta_e_2000(a, b) > 1.0);
    }
}
