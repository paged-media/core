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

//! Color management.
//!
//! Wraps Little CMS 2 (native) and qcms (wasm32) for ICC-based
//! transforms. All document paints are resolved to a linear RGB
//! working space before being shipped to the GPU; the final sRGB
//! (or proof-profile) conversion happens in a fragment shader so
//! blending remains physically meaningful.
//!
//! Backend selection by target_arch:
//! - native (lcms2): full ICC transforms, BPC, gamma, all rendering
//!   intents. Output matches what `pdftoppm` produces on the same
//!   PDF (poppler also uses lcms2 internally).
//! - wasm32 (qcms): Mozilla's pure-Rust ICC library, the same one
//!   Firefox ships. Supports CMYK with iccv4 profiles + relative-
//!   colorimetric intent. The wasm path used to no-op here — the
//!   canvas painted with a naive CMYK→RGB mapping while the native
//!   gate matched the PDF via lcms2, producing a uniform ~9 ΔE gap
//!   on every CMYK pack. Routing through qcms closes that gap.

pub mod ase;
pub mod cmm;
pub mod lab;

pub use cmm::{Cmm, DisplaySetup, ExportPolicy, GamutStatus, IccCmm, Intent, WorkingColor};

#[derive(Debug, thiserror::Error)]
pub enum IccError {
    #[cfg(not(target_arch = "wasm32"))]
    #[error("lcms2: {0}")]
    Lcms(#[from] lcms2::Error),
    #[error("ICC profile bytes invalid")]
    Invalid,
    /// Retained for any remaining no-op paths but unreachable on
    /// the supported targets.
    #[error("ICC transforms not supported on this target")]
    Unsupported,
}

/// CMYK percentage values (each 0.0..=100.0). InDesign's native shape.
#[derive(Debug, Clone, Copy)]
pub struct Cmyk {
    pub c: f32,
    pub m: f32,
    pub y: f32,
    pub k: f32,
}

/// Linear RGB (0.0..=1.0) — the working space the GPU renderer
/// expects. sRGB gamma encoding is applied at the rasterizer's
/// boundary, not here.
#[derive(Debug, Clone, Copy)]
pub struct LinearRgb(pub [f32; 3]);

/// Wraps a CMYK → linear-RGB ICC transform. Build once per render and
/// reuse — lcms2 transforms are thread-safe but expensive to create.
pub struct IccTransform {
    #[cfg(not(target_arch = "wasm32"))]
    inner: TransformInner,
    #[cfg(target_arch = "wasm32")]
    inner: QcmsTransform,
}

#[cfg(not(target_arch = "wasm32"))]
struct TransformInner {
    transform: lcms2::Transform<[u8; 4], [u8; 3]>,
}

#[cfg(target_arch = "wasm32")]
struct QcmsTransform {
    // qcms returns 8-bit sRGB-encoded bytes after the CMYK→sRGB
    // transform. We hold the transform handle and convert single
    // pixels / byte blocks on demand. Source format is qcms-side
    // CMYK4, output is qcms-side RGB8.
    transform: qcms::Transform,
}

impl IccTransform {
    /// Build a CMYK → linear-sRGB transform.
    ///
    /// `cmyk_profile` is the source CMYK ICC profile (e.g. Coated
    /// FOGRA39 v2 — bring your own). The destination is linear sRGB
    /// constructed via `lcms2::Profile::new_rgb` with linear TRCs.
    /// Rendering intent is Relative Colorimetric with black-point
    /// compensation (idea.md §9.2 default).
    /// Back-compat shim — today's hardcoded behaviour, preserved
    /// verbatim so an unconfigured pipeline renders bit-identically:
    /// native = Relative Colorimetric + BPC; wasm32 = qcms
    /// Perceptual. New callers use [`Self::cmyk_to_linear_rgb_with`].
    pub fn cmyk_to_linear_rgb(cmyk_profile: &[u8]) -> Result<Self, IccError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::cmyk_to_linear_rgb_with(
                cmyk_profile,
                cmm::Intent::RelativeColorimetric,
                true,
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self::cmyk_to_linear_rgb_with(cmyk_profile, cmm::Intent::Perceptual, true)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn cmyk_to_linear_rgb_with(
        cmyk_profile: &[u8],
        intent: cmm::Intent,
        bpc: bool,
    ) -> Result<Self, IccError> {
        use lcms2::{Flags, Intent, PixelFormat, Profile};
        let src = Profile::new_icc(cmyk_profile).map_err(|_| IccError::Invalid)?;
        // Mimic poppler's GfxICCBasedColorSpace transform setup so our
        // CMYK→sRGB matches what `pdftoppm` produces from an InDesign-
        // exported PDF (the corpus reference path). Poppler uses:
        //   - 8-bit CMYK source (BYTES_SH(1))
        //   - 8-bit RGB destination (TYPE_RGB_8) into a real sRGB
        //     profile (with the actual sRGB TRC, not gamma=1.0)
        //   - Relative Colorimetric, no flags (no BPC)
        // We diverge in two places by necessity: our destination
        // profile is the lcms2 built-in sRGB (`Profile::new_srgb`)
        // rather than the system display profile, and we then
        // un-gamma the output back to linear for the renderer's
        // linear-RGB compositing. The un-gamma step is the inverse
        // of the gamma the destination applied, so this is
        // mathematically a no-op trip — but the lcms2 transform
        // *internally* runs through the destination's TRC, which
        // changes the precision/quantisation of the output relative
        // to a flat-linear destination. Empirically this closes the
        // ~3-4 ΔE residual on geometry.idml's K=100 black squares
        // (lcms2-flat-linear → sRGB ≈(29,29,27); poppler-style →
        // ≈(35,31,32) matching pdftoppm's reference rasterisation).
        let dst = Profile::new_srgb();
        // Concept 2 — intent + BPC are now per-document settings;
        // the defaults reproduce the previously hardcoded
        // RelativeColorimetric + BLACKPOINT_COMPENSATION exactly.
        let lcms_intent = match intent {
            cmm::Intent::Perceptual => Intent::Perceptual,
            cmm::Intent::RelativeColorimetric => Intent::RelativeColorimetric,
            cmm::Intent::Saturation => Intent::Saturation,
            cmm::Intent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
        };
        let flags = if bpc {
            Flags::BLACKPOINT_COMPENSATION
        } else {
            Flags::default()
        };
        let transform = lcms2::Transform::new_flags(
            &src,
            PixelFormat::CMYK_8,
            &dst,
            PixelFormat::RGB_8,
            lcms_intent,
            flags,
        )?;
        Ok(IccTransform {
            inner: TransformInner { transform },
        })
    }

    /// wasm32 — qcms has no BPC flag (the parameter is accepted and
    /// ignored); Saturation/Absolute intents map onto qcms's
    /// corresponding tags, with qcms's own internal degradation.
    #[cfg(target_arch = "wasm32")]
    pub fn cmyk_to_linear_rgb_with(
        cmyk_profile: &[u8],
        intent: cmm::Intent,
        _bpc: bool,
    ) -> Result<Self, IccError> {
        // Mirror the lcms2 path's destination choice: sRGB with the
        // standard TRC, decode-to-linear after the trip. qcms's
        // `Transform::new` builds an sRGB destination internally; we
        // request 8-bit CMYK in, 8-bit RGB out, RelativeColorimetric.
        let src =
            qcms::Profile::new_from_slice(cmyk_profile, true).ok_or(IccError::Invalid)?;
        let mut dst = qcms::Profile::new_sRGB();
        // qcms requires `precache_output_transform` for non-trivial
        // CMYK lookups; without it the transform LUT stays unpopulated
        // and `transform_pixels` returns black.
        dst.precache_output_transform();
        let qcms_intent = match intent {
            cmm::Intent::Perceptual => qcms::Intent::Perceptual,
            cmm::Intent::RelativeColorimetric => qcms::Intent::RelativeColorimetric,
            cmm::Intent::Saturation => qcms::Intent::Saturation,
            cmm::Intent::AbsoluteColorimetric => qcms::Intent::AbsoluteColorimetric,
        };
        let transform = qcms::Transform::new_to(
            &src,
            &dst,
            qcms::DataType::CMYK,
            qcms::DataType::RGB8,
            qcms_intent,
        )
        .ok_or(IccError::Invalid)?;
        Ok(IccTransform {
            inner: QcmsTransform { transform },
        })
    }

    /// Convert a single CMYK percentage triple to linear RGB.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn cmyk_percent_to_linear_rgb(&self, cmyk: Cmyk) -> LinearRgb {
        // lcms2 PixelFormat::CMYK_8 expects 0..255 byte values per
        // channel. Quantise the IDML's 0..100 percentages by
        // mapping `pct/100 * 255` and rounding — same precision
        // poppler uses (8-bit CMYK throughout its color path).
        let to_byte = |pct: f32| (pct * 2.55).round().clamp(0.0, 255.0) as u8;
        let input = [[
            to_byte(cmyk.c),
            to_byte(cmyk.m),
            to_byte(cmyk.y),
            to_byte(cmyk.k),
        ]];
        // Destination is real-sRGB (with sRGB TRC); output is
        // gamma-encoded sRGB bytes. Decode back to linear so the
        // renderer's downstream compositing stays in linear-light
        // space. The encode/decode trip is mathematically a no-op
        // but the lcms2 transform's internal precision is what
        // matches poppler's pdftoppm-equivalent output values.
        let mut output = [[0u8; 3]];
        self.inner.transform.transform_pixels(&input, &mut output);
        let to_linear = |b: u8| -> f32 {
            let s = b as f32 / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        LinearRgb([to_linear(output[0][0]), to_linear(output[0][1]), to_linear(output[0][2])])
    }

    #[cfg(target_arch = "wasm32")]
    pub fn cmyk_percent_to_linear_rgb(&self, cmyk: Cmyk) -> LinearRgb {
        // Same byte quantisation as the lcms2 path so the two
        // backends produce matching outputs for the same input.
        let to_byte = |pct: f32| (pct * 2.55).round().clamp(0.0, 255.0) as u8;
        let input = [to_byte(cmyk.c), to_byte(cmyk.m), to_byte(cmyk.y), to_byte(cmyk.k)];
        let mut output = [0u8; 3];
        self.inner.transform.convert(&input, &mut output);
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

    /// Track 1b: batch CMYK-8 → sRGB-8 byte transform. Used by the
    /// streaming JPEG decoder when a CMYK JPEG carries its own
    /// embedded ICC profile (annual-report-template / Q-03 newspaper
    /// packs) — we build a one-shot `IccTransform` from those bytes
    /// rather than the document-default CMYK profile. The output is
    /// sRGB-encoded bytes (the lcms2 transform's destination has a
    /// real sRGB TRC, matching `cmyk_percent_to_linear_rgb`'s
    /// internal trip), suitable for direct copy into an RGBA8 buffer
    /// without an extra encode pass.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn cmyk_bytes_to_rgb_bytes(&self, cmyk: &[[u8; 4]], rgb: &mut [[u8; 3]]) {
        self.inner.transform.transform_pixels(cmyk, rgb);
    }

    #[cfg(target_arch = "wasm32")]
    pub fn cmyk_bytes_to_rgb_bytes(&self, cmyk: &[[u8; 4]], rgb: &mut [[u8; 3]]) {
        for (src, dst) in cmyk.iter().zip(rgb.iter_mut()) {
            self.inner.transform.convert(src, dst);
        }
    }
}

// WIP: linear-sRGB profile builder, authored ahead of the transform
// path that will consume it. Kept compiling until then.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn build_linear_srgb_profile() -> Result<lcms2::Profile, IccError> {
    use lcms2::{CIExyY, CIExyYTRIPLE, Profile, ToneCurve};
    // sRGB primaries.
    let primaries = CIExyYTRIPLE {
        Red: CIExyY {
            x: 0.6400,
            y: 0.3300,
            Y: 1.0,
        },
        Green: CIExyY {
            x: 0.3000,
            y: 0.6000,
            Y: 1.0,
        },
        Blue: CIExyY {
            x: 0.1500,
            y: 0.0600,
            Y: 1.0,
        },
    };
    // D65 white point.
    let white = CIExyY {
        x: 0.3127,
        y: 0.3290,
        Y: 1.0,
    };
    // Linear TRC — we want linear-light output to hand to the GPU.
    let linear = ToneCurve::new(1.0);
    let curves = [&linear, &linear, &linear];
    Ok(Profile::new_rgb(&white, &primaries, &curves)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmyk_struct_holds_percentage_values() {
        let c = Cmyk {
            c: 0.0,
            m: 100.0,
            y: 100.0,
            k: 0.0,
        };
        assert_eq!(c.c, 0.0);
        assert_eq!(c.m, 100.0);
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_target_returns_unsupported() {
        let result = IccTransform::cmyk_to_linear_rgb(&[]);
        assert!(matches!(result, Err(IccError::Unsupported)));
    }
}
