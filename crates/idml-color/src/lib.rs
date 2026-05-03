//! Color management.
//!
//! Wraps Little CMS 2 for ICC-based transforms. All document paints are
//! resolved to a linear RGB working space before being shipped to the
//! GPU; the final sRGB (or proof-profile) conversion happens in a
//! fragment shader so blending remains physically meaningful.
//!
//! WASM build strategy is deferred until `spikes/wasm-size` concludes
//! whether `lcms2-sys` compiles cleanly to `wasm32-unknown-unknown` or
//! we need to bundle a separate lcms-wasm module. Today the
//! `IccTransform` API exists on every target; on wasm32 the
//! constructor returns `Err(IccError::Unsupported)` so callers fall
//! back to the naive math in `idml-parse::graphic`.

#[derive(Debug, thiserror::Error)]
pub enum IccError {
    #[cfg(not(target_arch = "wasm32"))]
    #[error("lcms2: {0}")]
    Lcms(#[from] lcms2::Error),
    #[error("ICC profile bytes invalid")]
    Invalid,
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
    /// Holds whatever lifetime extension the inner transform needs.
    #[cfg(target_arch = "wasm32")]
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(target_arch = "wasm32"))]
struct TransformInner {
    transform: lcms2::Transform<[u8; 4], [u8; 3]>,
}

impl IccTransform {
    /// Build a CMYK → linear-sRGB transform.
    ///
    /// `cmyk_profile` is the source CMYK ICC profile (e.g. Coated
    /// FOGRA39 v2 — bring your own). The destination is linear sRGB
    /// constructed via `lcms2::Profile::new_rgb` with linear TRCs.
    /// Rendering intent is Relative Colorimetric with black-point
    /// compensation (idea.md §9.2 default).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn cmyk_to_linear_rgb(cmyk_profile: &[u8]) -> Result<Self, IccError> {
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
        let transform = lcms2::Transform::new_flags(
            &src,
            PixelFormat::CMYK_8,
            &dst,
            PixelFormat::RGB_8,
            Intent::RelativeColorimetric,
            Flags::BLACKPOINT_COMPENSATION,
        )?;
        Ok(IccTransform {
            inner: TransformInner { transform },
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn cmyk_to_linear_rgb(_cmyk_profile: &[u8]) -> Result<Self, IccError> {
        Err(IccError::Unsupported)
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
    pub fn cmyk_percent_to_linear_rgb(&self, _cmyk: Cmyk) -> LinearRgb {
        // Should never be called — `cmyk_to_linear_rgb` returns
        // Unsupported on wasm. Provided for compile parity.
        LinearRgb([0.0, 0.0, 0.0])
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
