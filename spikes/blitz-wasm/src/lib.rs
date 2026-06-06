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

//! W0 — parse + style + layout + paint a static HTML fragment through
//! the Blitz stack into a command-counting `PaintScene`. The questions
//! this answers (paged.web concept §10 Q1):
//!
//! 1. does the Stylo-based stack COMPILE to `wasm32-unknown-unknown`?
//! 2. what is the marginal binary size (Vello excluded — shared with
//!    `paged-sdk` in a real integration)?
//! 3. does the paint path actually emit commands for a representative
//!    fragment (flexbox + borders + backgrounds + text)?

use anyrender::{Glyph, NormalizedCoord, PaintRef, PaintScene, RenderContext};
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use kurbo::{Affine, Rect, Shape, Stroke, Vec2};
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};

/// Counts every command kind `blitz-paint` pushes — enough to prove
/// the full parse→style→layout→paint path runs without a GPU.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RenderStats {
    pub fills: usize,
    pub strokes: usize,
    pub glyph_runs: usize,
    pub layers: usize,
    pub box_shadows: usize,
}

impl RenderStats {
    pub fn total(&self) -> usize {
        self.fills + self.strokes + self.glyph_runs + self.layers + self.box_shadows
    }
}

#[derive(Default)]
struct CountingScene {
    stats: RenderStats,
}

// All of `RenderContext`'s methods have defaults — the counting scene
// registers no custom resources.
impl RenderContext for CountingScene {}

impl PaintScene for CountingScene {
    fn reset(&mut self) {
        self.stats = RenderStats::default();
    }

    fn push_layer(
        &mut self,
        _blend: impl Into<BlendMode>,
        _alpha: f32,
        _transform: Affine,
        _clip: &impl Shape,
    ) {
        self.stats.layers += 1;
    }

    fn push_clip_layer(&mut self, _transform: Affine, _clip: &impl Shape) {
        self.stats.layers += 1;
    }

    fn pop_layer(&mut self) {}

    fn stroke<'a>(
        &mut self,
        _style: &Stroke,
        _transform: Affine,
        _brush: impl Into<PaintRef<'a>>,
        _brush_transform: Option<Affine>,
        _shape: &impl Shape,
    ) {
        self.stats.strokes += 1;
    }

    fn fill<'a>(
        &mut self,
        _style: Fill,
        _transform: Affine,
        _brush: impl Into<PaintRef<'a>>,
        _brush_transform: Option<Affine>,
        _shape: &impl Shape,
    ) {
        self.stats.fills += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        _font: &'a FontData,
        _font_size: f32,
        _hint: bool,
        _normalized_coords: &'a [NormalizedCoord],
        _embolden: Vec2,
        _style: impl Into<StyleRef<'a>>,
        _brush: impl Into<PaintRef<'a>>,
        _brush_alpha: f32,
        _transform: Affine,
        _glyph_transform: Option<Affine>,
        _glyphs: impl Iterator<Item = Glyph> + Clone,
    ) {
        self.stats.glyph_runs += 1;
    }

    fn draw_box_shadow(
        &mut self,
        _transform: Affine,
        _rect: Rect,
        _brush: Color,
        _radius: f64,
        _std_dev: f64,
    ) {
        self.stats.box_shadows += 1;
    }
}

/// The representative fragment: flexbox row, borders, backgrounds,
/// nested block flow, text — the §6 "placed content frame" shape.
pub const FRAGMENT: &str = r#"<!DOCTYPE html>
<html><head><style>
  body { margin: 0; background: #eef1f5; }
  .card { display: flex; gap: 12px; padding: 16px;
          border: 2px solid #314158; background: #ffffff; }
  .badge { width: 64px; height: 64px; background: #0a6e8a;
           border-radius: 8px; }
  .body { flex: 1; border-left: 4px solid #b80a52; padding-left: 12px; }
  h1 { font-size: 20px; margin: 0 0 8px 0; color: #15202b; }
  p { margin: 0; font-size: 13px; line-height: 1.5; color: #3d4c5c; }
</style></head>
<body>
  <div class="card">
    <div class="badge"></div>
    <div class="body">
      <h1>paged.web W0 spike</h1>
      <p>HTML and CSS, laid out by Stylo and Taffy, painted through
         the anyrender abstraction — no browser, no server.</p>
    </div>
  </div>
</body></html>"#;

/// Parse → style → layout → paint at `width`×`height` (CSS px) and
/// return the command counts.
pub fn render_fragment(html: &str, width: u32, height: u32) -> RenderStats {
    let mut doc = HtmlDocument::from_html(html, DocumentConfig::default());
    doc.set_viewport(Viewport::new(width, height, 1.0, ColorScheme::Light));
    doc.resolve(0.0);
    let mut scene = CountingScene::default();
    paint_scene(&mut scene, &mut doc, 1.0, width, height, 0, 0);
    scene.stats
}

/// Bench helper — repaint an already-resolved document and return the
/// command count (examples/w0_bench.rs).
pub fn paint_count(doc: &mut blitz_dom::BaseDocument, width: u32, height: u32) -> usize {
    let mut scene = CountingScene::default();
    paint_scene(&mut scene, doc, 1.0, width, height, 0, 0);
    scene.stats.total()
}

/// Size-measurement anchor — keeps the full parse→style→layout→paint
/// stack reachable in the cdylib so `wasm-opt`'s DCE can't strip it.
/// On wasm32 it is also the RUNTIME proof: executed in node by
/// `npm-run`/manual `wasm-bindgen --target nodejs` (W0 finding #4).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
#[cfg_attr(not(target_arch = "wasm32"), no_mangle)]
pub extern "C" fn w0_render_fragment_command_count() -> u32 {
    render_fragment(FRAGMENT, 480, 320).total() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_paints_boxes_and_text() {
        let stats = render_fragment(FRAGMENT, 480, 320);
        println!("W0 stats: {stats:?}");
        // Backgrounds (body, card, badge) + borders must paint…
        assert!(stats.fills >= 3, "expected box fills, got {stats:?}");
        // …and the headline + paragraph must shape into glyph runs
        // (Parley with its bundled fallback fonts).
        assert!(stats.glyph_runs >= 1, "expected text, got {stats:?}");
    }

    #[test]
    fn empty_document_paints_only_the_root_canvas() {
        // Measured baseline: 3 fills + 2 layers (root canvas/background
        // plumbing). The interesting property is RELATIVE: real content
        // adds commands on top of this floor.
        let empty = render_fragment("<html><body></body></html>", 480, 320);
        assert!(empty.total() <= 6, "baseline drifted: {empty:?}");
        let full = render_fragment(FRAGMENT, 480, 320);
        assert!(full.total() > empty.total());
    }
}
