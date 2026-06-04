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

//! Concept 2, decision C3 — the CMM "bake-off", as a measurement.
//!
//! The renderer ships lcms2 on native and qcms on wasm32. Their
//! outputs must agree closely enough that the editor's live canvas
//! (qcms) is a faithful preview of the fidelity-gate reference
//! (lcms2, which matches poppler/pdftoppm). This test runs BOTH
//! CMMs natively over the same CMYK working-space profile and a
//! patch set covering the primaries, mixes, and the rich/pure black
//! distinction, and bounds the per-patch ΔE*ab.
//!
//! Profile-gated: looks for a CMYK profile via `PAGED_CMYK_PROFILE`,
//! `corpus/profiles/*.icc`, then the Adobe-installed Coated FOGRA39
//! path. Skips (passes with a notice) when none is present — the
//! corpus is a partial private checkout locally; CI provides one.

#![cfg(not(target_arch = "wasm32"))]

use paged_color::{lab, Cmyk, IccTransform, LinearRgb};

/// Patch set: IDML-style CMYK percentages.
const PATCHES: &[[f32; 4]] = &[
    [0.0, 0.0, 0.0, 0.0],       // paper
    [100.0, 0.0, 0.0, 0.0],     // pure cyan
    [0.0, 100.0, 0.0, 0.0],     // pure magenta
    [0.0, 0.0, 100.0, 0.0],     // pure yellow
    [0.0, 0.0, 0.0, 100.0],     // pure black (THE 100%-K patch)
    [60.0, 40.0, 40.0, 100.0],  // rich black
    [50.0, 0.0, 100.0, 0.0],    // green mix
    [0.0, 80.0, 95.0, 0.0],     // warm red
    [20.0, 20.0, 20.0, 20.0],   // muddy quad
    [100.0, 100.0, 0.0, 0.0],   // violet-blue
    [10.0, 5.0, 5.0, 0.0],      // near-paper tint
    [50.0, 50.0, 50.0, 50.0],   // mid quad
];

/// Acceptance bound, ΔE*ab per patch. The two CMMs share the same
/// 8-bit quantisation and sRGB destination; residuals come from
/// LUT interpolation differences. 2.5 is comfortably below a
/// just-noticeable difference for side-by-side canvas vs export.
const MAX_DELTA_E: f32 = 2.5;

fn find_profile() -> Option<Vec<u8>> {
    if let Ok(p) = std::env::var("PAGED_CMYK_PROFILE") {
        if let Ok(bytes) = std::fs::read(&p) {
            return Some(bytes);
        }
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    let corpus = std::path::Path::new(manifest).join("../../corpus/profiles");
    if let Ok(entries) = std::fs::read_dir(&corpus) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("icc")) {
                if let Ok(bytes) = std::fs::read(&path) {
                    return Some(bytes);
                }
            }
        }
    }
    // Adobe's recommended-profiles install (present on design
    // workstations; the same file the fidelity harness hands to
    // pdftoppm).
    let adobe = "/Library/Application Support/Adobe/Color/Profiles/Recommended/CoatedFOGRA39.icc";
    std::fs::read(adobe).ok()
}

/// qcms CMYK→sRGB→linear, mirroring `IccTransform`'s wasm path
/// byte-for-byte (same quantisation, same intent default, same
/// linear decode) so the comparison measures the CMM, not the glue.
struct QcmsNative {
    transform: qcms::Transform,
}

impl QcmsNative {
    fn new(profile: &[u8]) -> Option<Self> {
        let src = qcms::Profile::new_from_slice(profile, true)?;
        let mut dst = qcms::Profile::new_sRGB();
        dst.precache_output_transform();
        let transform = qcms::Transform::new_to(
            &src,
            &dst,
            qcms::DataType::CMYK,
            qcms::DataType::RGB8,
            qcms::Intent::Perceptual,
        )?;
        Some(Self { transform })
    }

    fn convert(&self, cmyk: Cmyk) -> LinearRgb {
        let to_byte = |pct: f32| (pct * 2.55).round().clamp(0.0, 255.0) as u8;
        let input = [to_byte(cmyk.c), to_byte(cmyk.m), to_byte(cmyk.y), to_byte(cmyk.k)];
        let mut output = [0u8; 3];
        self.transform.convert(&input, &mut output);
        let to_linear = |b: u8| -> f32 {
            let s = b as f32 / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        LinearRgb([to_linear(output[0]), to_linear(output[1]), to_linear(output[2])])
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn delta_e(a: LinearRgb, b: LinearRgb) -> f32 {
    let enc = |LinearRgb(c): LinearRgb| {
        [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2])]
    };
    let la = lab::srgb_to_lab_d50(enc(a));
    let lb = lab::srgb_to_lab_d50(enc(b));
    let d = [la[0] - lb[0], la[1] - lb[1], la[2] - lb[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

#[test]
fn lcms2_and_qcms_agree_on_cmyk_patches() {
    let Some(profile) = find_profile() else {
        eprintln!(
            "parity: no CMYK profile found (PAGED_CMYK_PROFILE / corpus/profiles / Adobe path) — skipping"
        );
        return;
    };
    let lcms = IccTransform::cmyk_to_linear_rgb(&profile).expect("lcms2 transform");
    let qcms = QcmsNative::new(&profile).expect("qcms transform");

    let mut worst: (f32, [f32; 4]) = (0.0, [0.0; 4]);
    for &[c, m, y, k] in PATCHES {
        let cmyk = Cmyk { c, m, y, k };
        let a = lcms.cmyk_percent_to_linear_rgb(cmyk);
        let b = qcms.convert(cmyk);
        let de = delta_e(a, b);
        eprintln!("parity: CMYK({c:>5.1},{m:>5.1},{y:>5.1},{k:>5.1})  dE*ab = {de:.2}");
        if de > worst.0 {
            worst = (de, [c, m, y, k]);
        }
        assert!(
            de <= MAX_DELTA_E,
            "lcms2 vs qcms diverge on CMYK({c},{m},{y},{k}): dE {de:.2} > {MAX_DELTA_E}"
        );
    }
    eprintln!(
        "parity: worst patch CMYK{:?} dE*ab = {:.2} (bound {MAX_DELTA_E})",
        worst.1, worst.0
    );
}

/// The canonical data-loss bug from the concept doc: a 100%-K text
/// black must stay visually black through BOTH CMMs (not drift
/// toward a washed four-colour grey on either path).
#[test]
fn pure_black_stays_black_on_both_cmms() {
    let Some(profile) = find_profile() else {
        eprintln!("parity: no CMYK profile — skipping");
        return;
    };
    let black = Cmyk { c: 0.0, m: 0.0, y: 0.0, k: 100.0 };
    let lcms = IccTransform::cmyk_to_linear_rgb(&profile).expect("lcms2 transform");
    let qcms = QcmsNative::new(&profile).expect("qcms transform");
    for (name, LinearRgb(rgb)) in [
        ("lcms2", lcms.cmyk_percent_to_linear_rgb(black)),
        ("qcms", qcms.convert(black)),
    ] {
        // Coated K=100 prints around L* 16-20; linear-light values
        // must stay well below 10% on every channel.
        assert!(
            rgb.iter().all(|v| *v < 0.10),
            "{name}: K=100 resolved to {rgb:?} — not black"
        );
    }
}
