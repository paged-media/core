//! Vello backend (stub).
//!
//! `PathRasterizer` impl that will eventually drive Vello-via-wgpu.
//! Currently a placeholder that returns the background colour as a
//! flat fill so the trait surface compiles + the pipeline can pick
//! the backend feature without crashing.
//!
//! Real integration plan (per Spike A's findings):
//!  1. Lazy-init a wgpu Adapter / Device / Queue on first
//!     `rasterize` call; cache for the rasterizer's lifetime.
//!  2. Walk the DisplayList building a `vello::Scene`:
//!     FillPath → `scene.fill(...)`,
//!     StrokePath → `scene.stroke(...)`,
//!     DropShadow → blur layer (Vello 0.8+),
//!     Image → `scene.draw_image(...)`.
//!  3. Render the scene to a wgpu texture sized to
//!     `options.pixel_size()`, read back to RGBA8.
//!  4. Map any DisplayCommand variants Vello can't yet render to
//!     a tiny-skia fallback path so the same DisplayList can mix
//!     backends per command (matching the plan's "fork only if
//!     blocked" stance).
//!
//! Pull-through to land:
//!  - GPU device + queue lifecycle (single shared instance).
//!  - DisplayList → Scene walker.
//!  - Glyph cache that survives across renders (per font_id).
//!  - Vello readback → RGBA8 in linear space (the pipeline's colour
//!    convention).

use idml_compose::DisplayList;

use crate::{PathRasterizer, RasterOptions};

#[derive(Debug, Default, Clone, Copy)]
pub struct VelloRasterizer;

impl PathRasterizer for VelloRasterizer {
    fn name(&self) -> &'static str {
        "vello/wgpu (stub)"
    }

    fn rasterize(&self, _list: &DisplayList, options: &RasterOptions) -> Vec<u8> {
        // Stub: paint the canvas with the background colour so the
        // output shape is correct for callers wiring this up. Real
        // command dispatch lands in the follow-up batch.
        let (w, h) = options.pixel_size();
        let bg = options.background;
        let r = (bg.r.clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = (bg.g.clamp(0.0, 1.0) * 255.0).round() as u8;
        let b = (bg.b.clamp(0.0, 1.0) * 255.0).round() as u8;
        let a = (bg.a.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&[r, g, b, a]);
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idml_compose::Color;

    #[test]
    fn stub_fills_background() {
        let v = VelloRasterizer;
        let mut opts = RasterOptions::new(10.0, 10.0);
        opts.dpi = 72.0;
        opts.background = Color::rgba(0.5, 0.0, 0.0, 1.0);
        let buf = v.rasterize(&DisplayList::new(), &opts);
        assert_eq!(buf.len(), 10 * 10 * 4);
        // Sample any pixel — they're all the background.
        assert!(buf[0] > 100 && buf[0] < 160);
        assert_eq!(buf[1], 0);
    }
}
