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

#[cfg(feature = "cpu")]
pub mod cpu;
#[cfg(all(feature = "vello-backend", target_arch = "wasm32"))]
pub mod surface;
#[cfg(feature = "vello-backend")]
pub mod vello_rs;

#[cfg(feature = "cpu")]
pub use cpu::rasterize;
#[cfg(feature = "cpu")]
pub use cpu::CpuRasterizer;
#[cfg(all(feature = "vello-backend", target_arch = "wasm32"))]
pub use surface::{SurfaceError, SurfacePresenter, Viewport};
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
