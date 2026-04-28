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
    transform: lcms2::Transform<[f32; 4], [f32; 3]>,
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
        let dst = build_linear_srgb_profile()?;
        let transform = lcms2::Transform::new_flags(
            &src,
            PixelFormat::CMYK_FLT,
            &dst,
            PixelFormat::RGB_FLT,
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
        // lcms2 PixelFormat::CMYK_FLT expects values on the 0..100
        // percentage scale (NOT 0..1 normalised; that produces near-
        // white output for every input). Pass the IDML percentages
        // through unchanged. Output values can fall slightly outside
        // [0,1] for out-of-gamut colours; clamp to the working space.
        let input = [[cmyk.c, cmyk.m, cmyk.y, cmyk.k]];
        let mut output = [[0.0f32; 3]];
        self.inner.transform.transform_pixels(&input, &mut output);
        LinearRgb([
            output[0][0].clamp(0.0, 1.0),
            output[0][1].clamp(0.0, 1.0),
            output[0][2].clamp(0.0, 1.0),
        ])
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
