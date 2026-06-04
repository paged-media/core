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

//! Concept 2 — analytic CIELAB(D50) → linear sRGB.
//!
//! IDML `Space="LAB"` swatches (and the freieFarbe HLC atlas, which
//! is defined in CIELAB) are device-independent: resolving them to
//! the display needs NO ICC profile, just the standard colorimetric
//! math — Lab(D50) → XYZ(D50) → Bradford chromatic adaptation
//! (D50→D65) → linear sRGB. Lab→CMYK (export) stays an ICC job
//! (Concept 3); this module is the display path that previously
//! returned `None` and dropped Lab swatches to grey.

use crate::LinearRgb;

/// CIE κ and ε (the exact rational forms).
const KAPPA: f32 = 24389.0 / 27.0;
const EPSILON: f32 = 216.0 / 24389.0;

/// D50 reference white (ICC PCS white point).
const D50: [f32; 3] = [0.964_22, 1.0, 0.825_21];

/// Bradford-adapted D50→D65 matrix (standard published values).
const BRADFORD_D50_TO_D65: [[f32; 3]; 3] = [
    [0.955_576_6, -0.023_039_3, 0.063_163_6],
    [-0.028_289_5, 1.009_941_6, 0.021_007_7],
    [0.012_298_2, -0.020_483_0, 1.329_909_8],
];

/// XYZ(D65) → linear sRGB (IEC 61966-2-1).
const XYZ_D65_TO_LINEAR_SRGB: [[f32; 3]; 3] = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266, 1.876_010_8, 0.041_556_0],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

fn mat_mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Lab(D50) → XYZ(D50).
fn lab_to_xyz_d50(l: f32, a: f32, b: f32) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let f_inv = |t: f32| {
        let t3 = t * t * t;
        if t3 > EPSILON {
            t3
        } else {
            (116.0 * t - 16.0) / KAPPA
        }
    };
    // The L* inverse uses the exact CIE form (handles L* below the
    // linear knee precisely).
    let yr = if l > KAPPA * EPSILON {
        let t = (l + 16.0) / 116.0;
        t * t * t
    } else {
        l / KAPPA
    };
    [f_inv(fx) * D50[0], yr * D50[1], f_inv(fz) * D50[2]]
}

/// CIELAB (D50) → linear sRGB, clamped to [0, 1]. Out-of-gamut Lab
/// values clip per-channel — the gamut WARNING is the CMM's
/// `check_gamut` job; display just clips like every other tool.
pub fn lab_d50_to_linear_srgb(l: f32, a: f32, b: f32) -> LinearRgb {
    let xyz_d50 = lab_to_xyz_d50(l, a, b);
    let xyz_d65 = mat_mul(&BRADFORD_D50_TO_D65, xyz_d50);
    let rgb = mat_mul(&XYZ_D65_TO_LINEAR_SRGB, xyz_d65);
    LinearRgb([
        rgb[0].clamp(0.0, 1.0),
        rgb[1].clamp(0.0, 1.0),
        rgb[2].clamp(0.0, 1.0),
    ])
}

/// CIELAB (D50) → sRGB-ENCODED components 0..=1 (gamma applied).
/// The wasm gamut probe round-trips through 8-bit sRGB and needs
/// the encoded form.
pub fn lab_d50_to_srgb_encoded(l: f32, a: f32, b: f32) -> [f32; 3] {
    let LinearRgb(lin) = lab_d50_to_linear_srgb(l, a, b);
    [
        linear_to_srgb(lin[0]),
        linear_to_srgb(lin[1]),
        linear_to_srgb(lin[2]),
    ]
}

/// sRGB-encoded (0..=1) → Lab(D50) — the reverse path the gamut
/// probe uses to compare round-trip endpoints in a perceptual
/// space.
pub fn srgb_to_lab_d50(srgb: [f32; 3]) -> [f32; 3] {
    let lin = [
        crate::cmm::srgb_to_linear(srgb[0]),
        crate::cmm::srgb_to_linear(srgb[1]),
        crate::cmm::srgb_to_linear(srgb[2]),
    ];
    // linear sRGB → XYZ(D65) (inverse of the published matrix).
    const LINEAR_SRGB_TO_XYZ_D65: [[f32; 3]; 3] = [
        [0.412_456_4, 0.357_576_1, 0.180_437_5],
        [0.212_672_9, 0.715_152_2, 0.072_175_0],
        [0.019_333_9, 0.119_192, 0.950_304_1],
    ];
    // Bradford D65→D50 (inverse adaptation).
    const BRADFORD_D65_TO_D50: [[f32; 3]; 3] = [
        [1.047_811_2, 0.022_886_6, -0.050_127_0],
        [0.029_542_4, 0.990_484_4, -0.017_049_1],
        [-0.009_234_5, 0.015_043_6, 0.752_131_6],
    ];
    let xyz_d65 = mat_mul(&LINEAR_SRGB_TO_XYZ_D65, lin);
    let xyz_d50 = mat_mul(&BRADFORD_D65_TO_D50, xyz_d65);
    let f = |t: f32| {
        if t > EPSILON {
            t.cbrt()
        } else {
            (KAPPA * t + 16.0) / 116.0
        }
    };
    let fx = f(xyz_d50[0] / D50[0]);
    let fy = f(xyz_d50[1] / D50[1]);
    let fz = f(xyz_d50[2] / D50[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn lab_white_is_srgb_white() {
        let LinearRgb(rgb) = lab_d50_to_linear_srgb(100.0, 0.0, 0.0);
        assert!(rgb.iter().all(|v| close(*v, 1.0, 2e-3)), "{rgb:?}");
    }

    #[test]
    fn lab_black_is_black() {
        let LinearRgb(rgb) = lab_d50_to_linear_srgb(0.0, 0.0, 0.0);
        assert!(rgb.iter().all(|v| close(*v, 0.0, 1e-4)), "{rgb:?}");
    }

    #[test]
    fn lab_mid_grey_matches_cie_y() {
        // L*=50 ⇒ Y = ((50+16)/116)^3 ≈ 0.18419 — the classic
        // "mid grey is 18%" anchor; neutral a*=b*=0 keeps all three
        // channels equal.
        let LinearRgb(rgb) = lab_d50_to_linear_srgb(50.0, 0.0, 0.0);
        assert!(close(rgb[0], 0.184_19, 2e-3), "{rgb:?}");
        assert!(close(rgb[0], rgb[1], 1e-4));
        assert!(close(rgb[1], rgb[2], 1e-3));
    }

    #[test]
    fn lab_srgb_red_round_trips() {
        // sRGB red's published Lab(D50) coordinates (Bradford-
        // adapted): approximately L*=54.29, a*=80.81, b*=69.89.
        let LinearRgb(rgb) = lab_d50_to_linear_srgb(54.29, 80.81, 69.89);
        assert!(close(rgb[0], 1.0, 2e-2), "{rgb:?}");
        assert!(close(rgb[1], 0.0, 2e-2), "{rgb:?}");
        assert!(close(rgb[2], 0.0, 2e-2), "{rgb:?}");
    }

    #[test]
    fn srgb_to_lab_inverts_lab_to_srgb() {
        // In-gamut value round-trips within quantisation noise.
        for (l, a, b) in [(60.0, 20.0, -30.0), (35.0, -15.0, 10.0), (80.0, 5.0, 60.0)] {
            let enc = lab_d50_to_srgb_encoded(l, a, b);
            let [l2, a2, b2] = srgb_to_lab_d50(enc);
            assert!(close(l, l2, 0.6), "L {l} vs {l2}");
            assert!(close(a, a2, 0.6), "a {a} vs {a2}");
            assert!(close(b, b2, 0.6), "b {b} vs {b2}");
        }
    }

    #[test]
    fn hlc_sample_lands_in_srgb() {
        // HLC H010_L50_C030 ≈ Lab(50, 29.5, 5.2) (hue 10°, chroma
        // 30). A mid-chroma atlas entry must resolve inside sRGB
        // (no channel pegged to the clamp).
        let LinearRgb(rgb) = lab_d50_to_linear_srgb(50.0, 29.5, 5.2);
        assert!(rgb.iter().all(|v| *v > 0.0 && *v < 1.0), "{rgb:?}");
    }
}
