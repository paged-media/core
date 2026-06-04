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

//! Concept 2 — the colour-management engine behind a narrow trait.
//!
//! [`Cmm`] is the ONLY surface panels / the renderer / (later) the
//! PDF exporter see; the lcms2-vs-qcms split stays inside this
//! crate. The trait operates on [`WorkingColor`] — a thin adapter
//! over the resolved channels of `paged_parse::ColorEntry` (the
//! IDML-faithful model stays canonical; this is NOT a new colour
//! type system).
//!
//! Intent + black-point compensation are CONSTRUCTOR state, not
//! per-call parameters: an ICC transform bakes its intent at build
//! time, and the document model is per-document settings (InDesign's
//! Color Settings) — per-object intent overrides can layer on later
//! by holding more than one `IccCmm`.

use crate::{Cmyk, IccTransform, LinearRgb};

/// A resolved colour value entering the CMM. Built by the caller
/// from `ColorEntry` (after `effective_cmyk()` folds spot
/// alternates + swatch tints) — see `paged-renderer`'s
/// `color_paint.rs` chokepoint.
#[derive(Debug, Clone, Copy)]
pub enum WorkingColor {
    /// CMYK percentages, 0..=100 per channel (IDML's native shape).
    Cmyk(Cmyk),
    /// CIELAB, D50 white point (IDML `Space="LAB"`).
    Lab { l: f32, a: f32, b: f32 },
    /// sRGB-encoded components 0..=1 (IDML stores 0..=255; divide
    /// before constructing).
    Rgb([f32; 3]),
    /// Gray ink percentage 0..=100 (0 = paper, 100 = solid).
    Gray(f32),
}

/// Rendering intent. Maps onto lcms2 natively; qcms accepts the
/// same four tags but its implementation is Perceptual/
/// RelativeColorimetric-centric — Saturation/Absolute degrade to
/// the nearest supported behaviour there (documented limitation,
/// measured by `tests/parity.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl Intent {
    /// Parse the IDML / wire spelling ("Perceptual",
    /// "RelativeColorimetric", …). Unknown ⇒ None.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Perceptual" | "perceptual" => Some(Self::Perceptual),
            "RelativeColorimetric" | "relativeColorimetric" => {
                Some(Self::RelativeColorimetric)
            }
            "Saturation" | "saturation" => Some(Self::Saturation),
            "AbsoluteColorimetric" | "absoluteColorimetric" => {
                Some(Self::AbsoluteColorimetric)
            }
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Perceptual => "Perceptual",
            Self::RelativeColorimetric => "RelativeColorimetric",
            Self::Saturation => "Saturation",
            Self::AbsoluteColorimetric => "AbsoluteColorimetric",
        }
    }
}

/// Per-document display-resolution settings. `Default` reproduces
/// the values that were hardcoded before this module existed
/// (Relative Colorimetric + BPC on native) so an unconfigured
/// pipeline renders bit-identically.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplaySetup {
    pub intent: Intent,
    pub bpc: bool,
}

impl Default for DisplaySetup {
    fn default() -> Self {
        Self {
            intent: Intent::RelativeColorimetric,
            bpc: true,
        }
    }
}

/// Out-of-gamut verdict for the mixer's warning triangle. A derived
/// read, never a binding kind (concept §5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GamutStatus {
    InGamut,
    OutOfGamut {
        /// Round-trip ΔE*ab against the destination space.
        delta_e: f32,
    },
}

/// The colour-management engine. One narrow trait; the
/// implementation (lcms2 native / qcms wasm) never leaks past it.
pub trait Cmm {
    /// Any [`WorkingColor`] → the linear-RGB working space the GPU
    /// expects. CMYK goes through the ICC transform when a working
    /// profile is configured (naive fallback otherwise — the
    /// pre-existing `to_linear_rgb` math, kept bit-identical); Lab
    /// resolves analytically (D50→D65 Bradford → linear sRGB);
    /// RGB/Gray decode analytically.
    fn resolve_display(&self, c: WorkingColor) -> LinearRgb;

    /// Out-of-gamut flag against the configured CMYK working space.
    fn check_gamut(&self, c: WorkingColor) -> GamutStatus;

    /// Concept-3 seam — export conversion preserving native spaces
    /// per policy. Identity until the PDF exporter lands; kept on
    /// the trait so the exporter compiles against the final shape.
    fn convert_for_export(&self, c: WorkingColor) -> WorkingColor;
}

/// The shipping `Cmm`: ICC for CMYK (lcms2 native / qcms wasm32),
/// analytic Lab/RGB/Gray. Build once per document settings change
/// and reuse — transforms are expensive to create.
pub struct IccCmm {
    cmyk: Option<IccTransform>,
    setup: DisplaySetup,
    /// M4 — round-trip transforms for `check_gamut` (built lazily
    /// from the same profile). None until a profile is configured.
    gamut: Option<GamutProbe>,
}

impl IccCmm {
    /// `cmyk_profile`: the document's CMYK working-space profile
    /// bytes (e.g. Coated FOGRA39). `None` ⇒ the naive fallback
    /// path — exactly today's unconfigured behaviour.
    pub fn new(cmyk_profile: Option<&[u8]>, setup: DisplaySetup) -> Self {
        let cmyk = cmyk_profile.and_then(|bytes| {
            match IccTransform::cmyk_to_linear_rgb_with(bytes, setup.intent, setup.bpc) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(error = %e, "CMYK working-space profile rejected; falling back to naive conversion");
                    None
                }
            }
        });
        let gamut = cmyk_profile.and_then(GamutProbe::new);
        Self { cmyk, setup, gamut }
    }

    pub fn setup(&self) -> DisplaySetup {
        self.setup
    }

    pub fn has_cmyk_profile(&self) -> bool {
        self.cmyk.is_some()
    }

    /// Borrow the underlying CMYK transform (the renderer's
    /// gradient tessellation batches conversions through it).
    pub fn cmyk_transform(&self) -> Option<&IccTransform> {
        self.cmyk.as_ref()
    }

    /// The naive CMYK→linear-RGB fallback — byte-for-byte the math
    /// `paged_parse::graphic::to_linear_rgb` has always used, so an
    /// unconfigured pipeline stays bit-identical.
    fn naive_cmyk(c: Cmyk) -> LinearRgb {
        let cv = c.c / 100.0;
        let mv = c.m / 100.0;
        let yv = c.y / 100.0;
        let kv = c.k / 100.0;
        let r = (1.0 - cv) * (1.0 - kv);
        let g = (1.0 - mv) * (1.0 - kv);
        let b = (1.0 - yv) * (1.0 - kv);
        LinearRgb([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)])
    }
}

impl Cmm for IccCmm {
    fn resolve_display(&self, c: WorkingColor) -> LinearRgb {
        match c {
            WorkingColor::Cmyk(cmyk) => match &self.cmyk {
                Some(t) => t.cmyk_percent_to_linear_rgb(cmyk),
                None => Self::naive_cmyk(cmyk),
            },
            WorkingColor::Lab { l, a, b } => crate::lab::lab_d50_to_linear_srgb(l, a, b),
            WorkingColor::Rgb([r, g, b]) => {
                LinearRgb([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)])
            }
            WorkingColor::Gray(pct) => {
                let g = srgb_to_linear(1.0 - pct / 100.0);
                LinearRgb([g, g, g])
            }
        }
    }

    fn check_gamut(&self, c: WorkingColor) -> GamutStatus {
        match &self.gamut {
            Some(probe) => probe.check(c),
            // No CMYK working space configured — nothing to be out
            // of gamut AGAINST; the mixer shows no warning.
            None => GamutStatus::InGamut,
        }
    }

    fn convert_for_export(&self, c: WorkingColor) -> WorkingColor {
        // Concept 3 — export policy (preserve native spaces /
        // Separation tint transforms) lands with the PDF backend.
        c
    }
}

/// sRGB EOTF decode — identical constants to
/// `paged_parse::graphic::srgb_to_linear` (duplicated by design:
/// parse must not depend on this crate, and the renderer needs the
/// same curve from both).
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Round-trip gamut probe against the CMYK working space (M4).
/// Native: lcms2 Lab↔CMYK transform pair; wasm32: qcms transform
/// pair. Built from the same profile bytes as the display
/// transform.
pub struct GamutProbe {
    #[cfg(not(target_arch = "wasm32"))]
    inner: native_gamut::Probe,
    #[cfg(target_arch = "wasm32")]
    inner: wasm_gamut::Probe,
}

impl GamutProbe {
    pub fn new(profile_bytes: &[u8]) -> Option<Self> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            native_gamut::Probe::new(profile_bytes).map(|inner| Self { inner })
        }
        #[cfg(target_arch = "wasm32")]
        {
            wasm_gamut::Probe::new(profile_bytes).map(|inner| Self { inner })
        }
    }

    fn check(&self, c: WorkingColor) -> GamutStatus {
        self.inner.check(c)
    }
}

/// Threshold above which a round-trip ΔE*ab counts as out of gamut.
/// 2.3 is the classic just-noticeable difference; we use 3.0 to
/// avoid flagging quantisation noise on 8-bit transform paths.
const GAMUT_DELTA_E_THRESHOLD: f32 = 3.0;

/// CIE76 ΔE*ab — sufficient for a warning triangle (CIEDE2000 is
/// overkill for a boolean gate; the fidelity harness uses ΔE2000
/// where it matters).
fn delta_e_76(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    (dl * dl + da * da + db * db).sqrt()
}

#[cfg(not(target_arch = "wasm32"))]
mod native_gamut {
    use super::{delta_e_76, GamutStatus, WorkingColor, GAMUT_DELTA_E_THRESHOLD};
    use crate::lab;

    /// Lab(D50) → CMYK(working space) → Lab(D50) round trip via
    /// lcms2; the ΔE between input and round-tripped Lab is the
    /// out-of-gamut measure.
    pub(super) struct Probe {
        to_cmyk: lcms2::Transform<[f32; 3], [f32; 4]>,
        to_lab: lcms2::Transform<[f32; 4], [f32; 3]>,
    }

    impl Probe {
        pub(super) fn new(profile_bytes: &[u8]) -> Option<Self> {
            use lcms2::{GlobalContext, Intent, PixelFormat, Profile};
            let cmyk = Profile::new_icc(profile_bytes).ok()?;
            // Lab4 PCS profile at the D50 white point (the ICC PCS).
            let lab = Profile::new_lab4_context(
                GlobalContext::new(),
                &lcms2::CIExyY {
                    x: 0.3457,
                    y: 0.3585,
                    Y: 1.0,
                },
            )
            .ok()?;
            // Relative colorimetric both ways — the round trip
            // measures gamut clipping, not intent rendering.
            let to_cmyk = lcms2::Transform::new(
                &lab,
                PixelFormat::Lab_FLT,
                &cmyk,
                PixelFormat::CMYK_FLT,
                Intent::RelativeColorimetric,
            )
            .ok()?;
            let to_lab = lcms2::Transform::new(
                &cmyk,
                PixelFormat::CMYK_FLT,
                &lab,
                PixelFormat::Lab_FLT,
                Intent::RelativeColorimetric,
            )
            .ok()?;
            Some(Self { to_cmyk, to_lab })
        }

        pub(super) fn check(&self, c: WorkingColor) -> GamutStatus {
            let lab_in = match working_to_lab(c) {
                Some(lab) => lab,
                // CMYK input is in its own gamut by definition.
                None => return GamutStatus::InGamut,
            };
            // lcms2 CMYK_FLT expects 0..100 per channel; Lab_FLT
            // expects real Lab ranges.
            let mut cmyk = [[0f32; 4]];
            self.to_cmyk.transform_pixels(&[lab_in], &mut cmyk);
            let mut lab_out = [[0f32; 3]];
            self.to_lab.transform_pixels(&cmyk, &mut lab_out);
            let de = delta_e_76(lab_in, lab_out[0]);
            if de > GAMUT_DELTA_E_THRESHOLD {
                GamutStatus::OutOfGamut { delta_e: de }
            } else {
                GamutStatus::InGamut
            }
        }
    }

    /// Resolve a WorkingColor to Lab(D50) for the round trip. CMYK
    /// returns None (already in the destination space).
    fn working_to_lab(c: WorkingColor) -> Option<[f32; 3]> {
        match c {
            WorkingColor::Cmyk(_) => None,
            WorkingColor::Lab { l, a, b } => Some([l, a, b]),
            WorkingColor::Rgb(rgb) => Some(lab::srgb_to_lab_d50(rgb)),
            WorkingColor::Gray(pct) => {
                Some(lab::srgb_to_lab_d50([1.0 - pct / 100.0; 3]))
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_gamut {
    use super::{delta_e_76, GamutStatus, WorkingColor, GAMUT_DELTA_E_THRESHOLD};
    use crate::lab;

    /// qcms has no Lab pixel format on the public API, so the wasm
    /// probe round-trips through sRGB instead: sRGB → CMYK(working)
    /// → sRGB via two qcms transforms, comparing in Lab (computed
    /// analytically from the sRGB endpoints). Coarser than the
    /// lcms2 probe but the same boolean gate.
    pub(super) struct Probe {
        to_cmyk: qcms::Transform,
        to_rgb: qcms::Transform,
    }

    impl Probe {
        pub(super) fn new(profile_bytes: &[u8]) -> Option<Self> {
            let cmyk = qcms::Profile::new_from_slice(profile_bytes, true)?;
            let mut srgb_dst = qcms::Profile::new_sRGB();
            srgb_dst.precache_output_transform();
            let srgb_src = qcms::Profile::new_sRGB();
            let to_cmyk = qcms::Transform::new_to(
                &srgb_src,
                &cmyk,
                qcms::DataType::RGB8,
                qcms::DataType::CMYK,
                qcms::Intent::RelativeColorimetric,
            )?;
            let cmyk_src = qcms::Profile::new_from_slice(profile_bytes, true)?;
            let to_rgb = qcms::Transform::new_to(
                &cmyk_src,
                &srgb_dst,
                qcms::DataType::CMYK,
                qcms::DataType::RGB8,
                qcms::Intent::RelativeColorimetric,
            )?;
            Some(Self { to_cmyk, to_rgb })
        }

        pub(super) fn check(&self, c: WorkingColor) -> GamutStatus {
            let srgb_in = match working_to_srgb(c) {
                Some(rgb) => rgb,
                None => return GamutStatus::InGamut,
            };
            let bytes_in = [
                (srgb_in[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (srgb_in[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (srgb_in[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            ];
            let mut cmyk = [0u8; 4];
            self.to_cmyk.convert(&bytes_in, &mut cmyk);
            let mut bytes_out = [0u8; 3];
            self.to_rgb.convert(&cmyk, &mut bytes_out);
            let srgb_out = [
                bytes_out[0] as f32 / 255.0,
                bytes_out[1] as f32 / 255.0,
                bytes_out[2] as f32 / 255.0,
            ];
            let de = delta_e_76(
                lab::srgb_to_lab_d50(srgb_in),
                lab::srgb_to_lab_d50(srgb_out),
            );
            if de > GAMUT_DELTA_E_THRESHOLD {
                GamutStatus::OutOfGamut { delta_e: de }
            } else {
                GamutStatus::InGamut
            }
        }
    }

    /// sRGB-encoded endpoint for the wasm round trip. Lab converts
    /// analytically; an sRGB source that clips during Lab→sRGB is
    /// already flagged by the clamp-distance shortcut below.
    fn working_to_srgb(c: WorkingColor) -> Option<[f32; 3]> {
        match c {
            WorkingColor::Cmyk(_) => None,
            WorkingColor::Lab { l, a, b } => Some(lab::lab_d50_to_srgb_encoded(l, a, b)),
            WorkingColor::Rgb(rgb) => Some(rgb),
            WorkingColor::Gray(pct) => Some([1.0 - pct / 100.0; 3]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_cmyk_matches_parse_layer_math() {
        // (1-c)(1-k) per channel, sRGB-decoded — the exact
        // pre-existing fallback. K=100 black:
        let LinearRgb(black) = IccCmm::naive_cmyk(Cmyk {
            c: 0.0,
            m: 0.0,
            y: 0.0,
            k: 100.0,
        });
        assert_eq!(black, [0.0, 0.0, 0.0]);
        // Paper white:
        let LinearRgb(white) = IccCmm::naive_cmyk(Cmyk {
            c: 0.0,
            m: 0.0,
            y: 0.0,
            k: 0.0,
        });
        assert_eq!(white, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn unconfigured_cmm_resolves_all_spaces() {
        let cmm = IccCmm::new(None, DisplaySetup::default());
        let LinearRgb(rgb) = cmm.resolve_display(WorkingColor::Rgb([1.0, 0.0, 0.0]));
        assert_eq!(rgb, [1.0, 0.0, 0.0]);
        let LinearRgb(gray) = cmm.resolve_display(WorkingColor::Gray(100.0));
        assert!(gray[0].abs() < 1e-6);
        let LinearRgb(lab_white) =
            cmm.resolve_display(WorkingColor::Lab { l: 100.0, a: 0.0, b: 0.0 });
        assert!(lab_white.iter().all(|v| (v - 1.0).abs() < 1e-3));
        // No working space configured ⇒ never out of gamut.
        assert_eq!(
            cmm.check_gamut(WorkingColor::Lab { l: 50.0, a: 120.0, b: -100.0 }),
            GamutStatus::InGamut
        );
    }

    #[test]
    fn intent_names_round_trip() {
        for i in [
            Intent::Perceptual,
            Intent::RelativeColorimetric,
            Intent::Saturation,
            Intent::AbsoluteColorimetric,
        ] {
            assert_eq!(Intent::from_name(i.name()), Some(i));
        }
        assert_eq!(Intent::from_name("UseColorSettings"), None);
    }
}
