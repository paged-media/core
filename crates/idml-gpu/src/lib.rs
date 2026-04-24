//! GPU backend.
//!
//! Exposes a `PathRasterizer` trait so the concrete rasterizer (Vello,
//! forked Vello, or a custom tile-based pipeline) can be swapped without
//! disturbing the rest of the pipeline. The choice is driven by Spike A
//! in `spikes/vello-eval`.

/// Abstraction over the rasterizer implementation.
///
/// The trait surface is deliberately small so Spike A's evaluation can
/// stand it up against multiple candidate backends cheaply.
pub trait PathRasterizer {
    /// Human-readable name, for logs and CI output.
    fn name(&self) -> &'static str;
}
