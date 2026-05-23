//! GPU backend.
//!
//! Owns the `PathRasterizer` trait — the abstraction that lets the
//! pipeline swap between rasterizer implementations (Vello, forked
//! Vello, or a custom tile-based pipeline) without changing callers.
//! The choice is driven by Spike A in `spikes/vello-eval`.
//!
//! Two impls live behind feature flags:
//!  - `cpu` (default): tiny-skia. Always-works backend used by tests
//!    and the fidelity harness.
//!  - `vello-backend`: Vello-via-wgpu. Currently a stub — real
//!    integration lands as a follow-up batch.

use idml_compose::{Color, DisplayList};

/// CMYK (each channel 0..=1) → linear-RGB via the naive Adobe-style
/// conversion (R=(1-C)(1-K) etc.) followed by sRGB→linear. Used by
/// rasterizers as the fallback when no ICC transform is plumbed
/// through. Matches the per-component math `idml-parse::graphic::to_linear_rgb`
/// applies for swatch-side CMYK, so a `Paint::Cmyk` of swatch values
/// rasterises identically to the prior `Paint::Solid` path for the
/// non-ICC case.
pub fn cmyk_unit_to_linear_rgb(c: f32, m: f32, y: f32, k: f32) -> Color {
    let c = c.clamp(0.0, 1.0);
    let m = m.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let k = k.clamp(0.0, 1.0);
    let r = (1.0 - c) * (1.0 - k);
    let g = (1.0 - m) * (1.0 - k);
    let b = (1.0 - y) * (1.0 - k);
    fn srgb_to_linear(v: f32) -> f32 {
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    Color::rgba(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0)
}

#[cfg(feature = "cpu")]
pub mod cpu;
#[cfg(all(feature = "vello-backend", target_arch = "wasm32"))]
pub mod surface;
#[cfg(feature = "vello-backend")]
pub mod vello_rs;
#[cfg(feature = "vello-backend")]
pub mod cmyk_compute;

#[cfg(feature = "cpu")]
pub use cpu::rasterize;
#[cfg(feature = "cpu")]
pub use cpu::CpuRasterizer;
#[cfg(all(feature = "vello-backend", target_arch = "wasm32"))]
pub use surface::{SurfaceError, SurfacePresenter, Viewport};
// Re-export vello::Scene so consumers (idml-canvas-wasm) don't need
// vello as a direct dependency to hold cached scenes.
#[cfg(all(feature = "vello-backend", target_arch = "wasm32"))]
pub use vello::Scene as VelloScene;
#[cfg(feature = "vello-backend")]
pub use vello_rs::VelloRasterizer;

/// Knobs every rasterizer respects.
#[derive(Debug, Clone, Copy)]
pub struct RasterOptions {
    /// Page width in pt.
    pub page_width_pt: f32,
    /// Page height in pt.
    pub page_height_pt: f32,
    /// Output DPI; 72 produces 1 px per pt, 300 is print quality.
    pub dpi: f32,
    /// Background fill applied to the whole canvas before any
    /// commands run. Linear RGB, as per display-list convention.
    pub background: Color,
}

impl RasterOptions {
    pub fn new(page_width_pt: f32, page_height_pt: f32) -> Self {
        Self {
            page_width_pt,
            page_height_pt,
            dpi: 96.0,
            background: Color::WHITE,
        }
    }

    /// Output dimensions in pixels at the configured DPI.
    pub fn pixel_size(&self) -> (u32, u32) {
        let scale = self.dpi / 72.0;
        let w = ((self.page_width_pt * scale).ceil() as u32).max(1);
        let h = ((self.page_height_pt * scale).ceil() as u32).max(1);
        (w, h)
    }
}

/// Rasterizer abstraction. Implementations turn a `DisplayList` into
/// an RGBA8 buffer at the requested DPI. The trait stays small so the
/// pipeline can pick a backend at construction time and the fidelity
/// harness can run multiple impls against the same display list.
///
/// Implementations should be cheap to construct — typically one per
/// render at most. Long-lived state (wgpu device, glyph atlas) lives
/// inside the impl.
pub trait PathRasterizer {
    /// Human-readable name; surfaces in CI logs and the inspect tool.
    fn name(&self) -> &'static str;

    /// Rasterise `list` at `options.dpi`. Returns an RGBA8 buffer of
    /// `options.pixel_size()` packed row-major. Implementations that
    /// can't currently render a particular `DisplayCommand` should
    /// log + skip it rather than fail the whole render.
    fn rasterize(&self, list: &DisplayList, options: &RasterOptions) -> Vec<u8>;
}
