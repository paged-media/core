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

//! WebGPU renderer-core session — the `@paged-media/sdk` surface.
//!
//! A small stateful **session** over `paged-renderer` (parse + layout) and
//! `paged-gpu` (Vello/WebGPU present + off-surface readback). It links
//! **nothing from the editor** (`paged-mutate`/`paged-canvas`/`paged-script`)
//! — the "sibling, not a shrunk app" boundary. It is the engine the public
//! viewer wraps and the docs `<live preview>` embeds.
//!
//! Forward rendering is **WebGPU-only**: there is no tiny-skia/CPU fallback
//! in this binary (`paged-renderer` is linked with `default-features = false`,
//! so its `cpu` rasterizer is excluded). When `navigator.gpu` is absent,
//! [`ViewerSession::new`] rejects and the consumer shows a "requires WebGPU"
//! message. The design spec is `WEBGPU.md` alongside this file.
//!
//! ```ts
//! import init, { ViewerSession } from '@paged-media/sdk';
//! await init();
//! const session = await ViewerSession.new();   // rejects if no WebGPU
//! const diags = session.load(idmlBytes, fontBytes);
//! await session.render_to_canvas(offscreenCanvas);
//! ```
//!
//! Native builds expose a plain library target so the crate can still
//! participate in `cargo check --workspace`.

mod build;
pub use build::viewer_build;

#[cfg(all(target_arch = "wasm32", feature = "gpu"))]
mod wasm {
    use paged_compose::Color;
    use paged_gpu::vello_kurbo::Affine;
    use paged_gpu::{SurfacePresenter, VelloScene};
    use paged_renderer::{BuiltDocument, BytesResolver, Document};
    use serde::Serialize;
    use tsify_next::Tsify;
    use wasm_bindgen::prelude::*;

    /// Vertical gap between pages in continuous layout, in pt — the
    /// same constant the editor canvas uses, so `page_layout()`
    /// offsets and `present()` placement always agree.
    const PAGE_GAP_PT: f32 = 24.0;

    #[wasm_bindgen(start)]
    pub fn on_start() {
        console_error_panic_hook::set_once();
        web_sys::console::log_1(&"paged-sdk: init".into());
    }

    /// Structured, JSON-serialisable result of `load` / `render_*`. Unlike
    /// the legacy free functions, recoverable parse/layout problems are
    /// reported here (so the docs preview can surface them inline) rather
    /// than thrown as an opaque `JsError`.
    #[derive(Serialize, Tsify)]
    #[tsify(into_wasm_abi)]
    pub struct Diagnostics {
        pub ok: bool,
        pub messages: Vec<Diagnostic>,
    }

    #[derive(Serialize, Tsify)]
    pub struct Diagnostic {
        /// `"error" | "warning" | "info"`.
        pub severity: String,
        /// Short machine code, e.g. `"open"`, `"build"`, `"no_gpu"`.
        pub code: String,
        pub message: String,
        /// IDML part the problem originates from, when known.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub part: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub line: Option<u32>,
    }

    impl Diagnostics {
        fn ok() -> Self {
            Self {
                ok: true,
                messages: Vec::new(),
            }
        }

        fn error(code: &str, message: &str) -> Self {
            Self {
                ok: false,
                messages: vec![Diagnostic {
                    severity: "error".to_string(),
                    code: code.to_string(),
                    message: message.to_string(),
                    part: None,
                    line: None,
                }],
            }
        }
    }

    /// Headless readback result. `rgba` is tightly-packed RGBA8
    /// (`width * height * 4`), surfaced to JS as a `Uint8Array`.
    #[wasm_bindgen(getter_with_clone)]
    pub struct RenderedRaster {
        pub width: u32,
        pub height: u32,
        pub rgba: Vec<u8>,
    }

    /// Continuous-layout geometry for every page, so the TS wrapper
    /// computes fit / goToPage / currentPage / scroll extents without
    /// wasm round-trips. `y_pt` is the page top in doc space (vertical
    /// stack with `gap_pt` between pages — the same offsets
    /// `present()` uses).
    #[derive(Serialize, Tsify)]
    #[tsify(into_wasm_abi)]
    #[serde(rename_all = "camelCase")]
    pub struct PagesLayout {
        pub gap_pt: f32,
        pub pages: Vec<PageRect>,
    }

    #[derive(Serialize, Tsify)]
    #[serde(rename_all = "camelCase")]
    pub struct PageRect {
        pub index: u32,
        pub y_pt: f32,
        pub width_pt: f32,
        pub height_pt: f32,
    }

    /// Renderer-core session. Holds the parsed+laid-out document and a
    /// lazily-created WebGPU presenter (device + surface). One per viewer
    /// instance / preview component.
    #[wasm_bindgen]
    pub struct ViewerSession {
        built: Option<BuiltDocument>,
        /// Created on first `render_to_canvas` (bound to that canvas) or
        /// lazily for headless `render_to_bytes` (1×1 backing canvas).
        presenter: Option<SurfacePresenter>,
        /// Per-(family, style) font payloads accumulated via
        /// `register_font`, consulted at `load` time. The same
        /// `BytesResolver` the engine, `paged-inspect`, and the editor use.
        fonts: BytesResolver,
        /// Current page index into `built.pages`.
        page: usize,
        /// Per-page Vello scenes, built on first visibility in
        /// `present()` and immutable thereafter (the viewer never
        /// mutates the document) — cleared on `load`. Unbounded by
        /// design for V1; revisit with an LRU if 100+-page documents
        /// show memory pressure.
        scenes: std::collections::HashMap<usize, VelloScene>,
    }

    #[wasm_bindgen]
    impl ViewerSession {
        /// Acquire the session, gating on WebGPU availability. Rejects when
        /// `navigator.gpu` is absent so the consumer can map that to the
        /// WebGPU-absent note. Exposed to JS as `ViewerSession.new()`
        /// returning a `Promise`.
        #[allow(clippy::new_without_default)]
        pub async fn new() -> Result<ViewerSession, JsError> {
            console_error_panic_hook::set_once();
            if !webgpu_available() {
                return Err(JsError::new(
                    "WebGPU is not available (navigator.gpu is undefined)",
                ));
            }
            Ok(ViewerSession {
                built: None,
                presenter: None,
                fonts: BytesResolver::new(),
                page: 0,
                scenes: std::collections::HashMap::new(),
            })
        }

        /// Register a font for an IDML `AppliedFont` family (and optional
        /// style such as `"Bold"` / `"Italic"`). Accumulates across calls
        /// and is consulted on the next `load`; real documents reference
        /// many faces, so call this once per face before `load`. Mirrors
        /// the editor's `RegisterFont` and `paged-inspect`'s
        /// `--font-family "Family[/Style]=PATH"`.
        pub fn register_font(&mut self, family: String, style: Option<String>, bytes: Box<[u8]>) {
            self.fonts
                .add_font(&family, style.as_deref(), bytes.into_vec());
        }

        /// Parse + build a complete IDML package. `font` is optional. Caches
        /// the built document and resets the current page to 0. Returns
        /// structured diagnostics; does not throw on recoverable problems.
        pub fn load(&mut self, idml: &[u8], font: Option<Box<[u8]>>) -> Diagnostics {
            let document = match Document::open(idml) {
                Ok(d) => d,
                Err(e) => return Diagnostics::error("open", &format!("open IDML: {e}")),
            };
            // `crate::viewer_build` is the single load path — shared
            // with the native digest-equivalence test ("same code,
            // same scene"). `font` is the last-resort fallback; the
            // registered `fonts` resolver handles per-family lookup.
            let built = match crate::viewer_build(&document, font.as_deref(), &self.fonts) {
                Ok(built) => built,
                Err(e) => return Diagnostics::error("build", &e),
            };
            self.built = Some(built);
            self.page = 0;
            self.scenes.clear();
            Diagnostics::ok()
        }

        pub fn page_count(&self) -> u32 {
            self.built
                .as_ref()
                .map(|b| b.pages.len() as u32)
                .unwrap_or(0)
        }

        pub fn set_page(&mut self, index: u32) {
            self.page = index as usize;
        }

        /// Present the current page to a worker-owned `OffscreenCanvas` via
        /// WebGPU. The presenter is created (bound to `canvas`) on the first
        /// call; later calls reuse it and re-present — pass the same canvas.
        pub async fn render_to_canvas(&mut self, canvas: web_sys::OffscreenCanvas) -> Diagnostics {
            if self.presenter.is_none() {
                let w = canvas.width().max(1);
                let h = canvas.height().max(1);
                match SurfacePresenter::new_offscreen(canvas, w, h).await {
                    Ok(p) => self.presenter = Some(p),
                    Err(e) => {
                        return Diagnostics::error("gpu_init", &format!("WebGPU init failed: {e}"))
                    }
                }
            }
            self.present_current()
        }

        /// Main-thread variant of [`Self::render_to_canvas`] for embedders
        /// that render on an `HtmlCanvasElement` instead of a worker
        /// `OffscreenCanvas`.
        pub async fn render_to_canvas_main(
            &mut self,
            canvas: web_sys::HtmlCanvasElement,
        ) -> Diagnostics {
            if self.presenter.is_none() {
                let w = canvas.width().max(1);
                let h = canvas.height().max(1);
                match SurfacePresenter::new(canvas, w, h).await {
                    Ok(p) => self.presenter = Some(p),
                    Err(e) => {
                        return Diagnostics::error("gpu_init", &format!("WebGPU init failed: {e}"))
                    }
                }
            }
            self.present_current()
        }

        /// Headless path: render the current page off-surface and read it
        /// back as RGBA8 (replaces the legacy `render_to_png`). For
        /// screenshots / SSR. `dpi` controls resolution (72 = 1px per pt).
        pub async fn render_to_bytes(&mut self, dpi: f32) -> Result<RenderedRaster, JsError> {
            self.ensure_presenter()
                .await
                .map_err(|e| JsError::new(&e))?;

            // Build the scaled scene while borrowing `built`, then drop the
            // borrow before taking `&mut presenter` for the async readback —
            // the borrow dance `paged-canvas-wasm::render_page_vello_png` uses.
            let (scene, width_px, height_px) = {
                let built = self
                    .built
                    .as_ref()
                    .ok_or_else(|| JsError::new("no document loaded"))?;
                let page = built
                    .pages
                    .get(self.page)
                    .ok_or_else(|| JsError::new("page index out of range"))?;
                let page_scene =
                    SurfacePresenter::build_page_scene(&page.list, page.width_pt, page.height_pt);
                let scale = (dpi / 72.0) as f64;
                let mut scene = VelloScene::new();
                scene.append(&page_scene, Some(Affine::scale(scale)));
                let width_px = ((page.width_pt * dpi / 72.0).ceil() as u32).max(1);
                let height_px = ((page.height_pt * dpi / 72.0).ceil() as u32).max(1);
                (scene, width_px, height_px)
            };

            let presenter = self
                .presenter
                .as_mut()
                .ok_or_else(|| JsError::new("GPU not initialised"))?;
            let rgba = presenter
                .render_scene_to_rgba(&scene, width_px, height_px)
                .await
                .map_err(|e| JsError::new(&format!("readback: {e}")))?;
            Ok(RenderedRaster {
                width: width_px,
                height: height_px,
                rgba,
            })
        }

        /// Resize the bound GPU surface. `width`/`height` are CSS pixels;
        /// `device_pixel_ratio` brings them to device pixels. No-op until a
        /// presenter exists.
        pub fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f32) {
            if let Some(p) = self.presenter.as_mut() {
                let w = ((width as f32 * device_pixel_ratio).round() as u32).max(1);
                let h = ((height as f32 * device_pixel_ratio).round() as u32).max(1);
                p.resize(w, h);
            }
        }

        /// Continuous-layout page geometry (doc-space pt, vertical
        /// stack with `PAGE_GAP_PT` between pages). Empty until `load`
        /// succeeds. The TS wrapper derives fit zoom, scroll extents,
        /// `goToPage` targets and the current page from this — no
        /// per-frame wasm round-trips.
        pub fn page_layout(&self) -> PagesLayout {
            let mut pages = Vec::new();
            let mut y_pt = 0.0_f32;
            if let Some(built) = self.built.as_ref() {
                for (index, page) in built.pages.iter().enumerate() {
                    pages.push(PageRect {
                        index: index as u32,
                        y_pt,
                        width_pt: page.width_pt,
                        height_pt: page.height_pt,
                    });
                    y_pt += page.height_pt + PAGE_GAP_PT;
                }
            }
            PagesLayout {
                gap_pt: PAGE_GAP_PT,
                pages,
            }
        }

        /// Camera-transformed present of the continuous page stack —
        /// the viewer's per-frame paint (ports the editor canvas's
        /// `presentFrame`). `zoom` is CSS px per pt; `scroll_x` /
        /// `scroll_y` place the doc origin in CSS px (positive moves
        /// content right/down); `dpr` brings CSS px to device px.
        /// Off-viewport pages are culled; per-page scenes build once
        /// and cache. `only_page` restricts the pass to one page laid
        /// out at y = 0 (the wrapper's `"single"` layout mode).
        ///
        /// Requires a bound presenter (any `render_to_canvas*` call
        /// binds one).
        pub fn present(
            &mut self,
            zoom: f32,
            scroll_x: f32,
            scroll_y: f32,
            dpr: f32,
            only_page: Option<u32>,
        ) -> Diagnostics {
            let Some(built) = self.built.as_ref() else {
                return Diagnostics::error("no_document", "no document loaded");
            };
            let Some(presenter) = self.presenter.as_ref() else {
                return Diagnostics::error("no_gpu", "GPU not initialised");
            };

            let k = zoom * dpr;
            let viewport_w = presenter.width() as f32;
            let viewport_h = presenter.height() as f32;

            // Pass 1: visibility-cull and make sure every visible page
            // has a cached scene (mut-borrows `self.scenes`).
            let mut visible: Vec<(usize, f32)> = Vec::new();
            let mut y_pt = 0.0_f32;
            for (idx, page) in built.pages.iter().enumerate() {
                let (skip, y_here) = match only_page {
                    Some(p) => (idx != p as usize, 0.0),
                    None => (false, y_pt),
                };
                y_pt += page.height_pt + PAGE_GAP_PT;
                if skip {
                    continue;
                }
                let top = scroll_y * dpr + y_here * k;
                let left = scroll_x * dpr;
                let on_screen = left + page.width_pt * k > 0.0
                    && left < viewport_w
                    && top + page.height_pt * k > 0.0
                    && top < viewport_h;
                if !on_screen {
                    continue;
                }
                self.scenes.entry(idx).or_insert_with(|| {
                    SurfacePresenter::build_page_scene(&page.list, page.width_pt, page.height_pt)
                });
                visible.push((idx, y_here));
            }

            // Pass 2: assemble (scene, transform) pairs from the cache
            // (shared borrows only) and present in one pass.
            let scene_list: Vec<(&VelloScene, [f32; 6])> = visible
                .iter()
                .filter_map(|(idx, y_here)| {
                    self.scenes.get(idx).map(|scene| {
                        (
                            scene,
                            [k, 0.0, 0.0, k, scroll_x * dpr, scroll_y * dpr + y_here * k],
                        )
                    })
                })
                .collect();

            let Some(presenter) = self.presenter.as_mut() else {
                return Diagnostics::error("no_gpu", "GPU not initialised");
            };
            let bg = Color::rgba(0.898, 0.905, 0.922, 1.0);
            match presenter.present_scenes(&scene_list, bg) {
                Ok(()) => Diagnostics::ok(),
                Err(e) => Diagnostics::error("present", &format!("present: {e}")),
            }
        }

        /// Headless render of ONE page scaled to `target_width_px`
        /// (thumbnails / page strips). Aspect ratio preserved.
        pub async fn render_page_to_bytes(
            &mut self,
            index: u32,
            target_width_px: u32,
        ) -> Result<RenderedRaster, JsError> {
            self.ensure_presenter()
                .await
                .map_err(|e| JsError::new(&e))?;

            let (scene, width_px, height_px) = {
                let built = self
                    .built
                    .as_ref()
                    .ok_or_else(|| JsError::new("no document loaded"))?;
                let page = built
                    .pages
                    .get(index as usize)
                    .ok_or_else(|| JsError::new("page index out of range"))?;
                let page_scene =
                    SurfacePresenter::build_page_scene(&page.list, page.width_pt, page.height_pt);
                let width_px = target_width_px.max(1);
                let scale = f64::from(width_px) / f64::from(page.width_pt.max(1.0));
                let height_px = ((f64::from(page.height_pt) * scale).ceil() as u32).max(1);
                let mut scene = VelloScene::new();
                scene.append(&page_scene, Some(Affine::scale(scale)));
                (scene, width_px, height_px)
            };

            let presenter = self
                .presenter
                .as_mut()
                .ok_or_else(|| JsError::new("GPU not initialised"))?;
            let rgba = presenter
                .render_scene_to_rgba(&scene, width_px, height_px)
                .await
                .map_err(|e| JsError::new(&format!("readback: {e}")))?;
            Ok(RenderedRaster {
                width: width_px,
                height: height_px,
                rgba,
            })
        }
    }

    // Non-exported helpers — kept out of the `#[wasm_bindgen]` impl.
    impl ViewerSession {
        /// Compose the current page's scene fit-and-centred into the bound
        /// surface and present it. Builds the scene under a short borrow of
        /// `built`, releases it, then presents with `&mut presenter`.
        fn present_current(&mut self) -> Diagnostics {
            let (scene, transform) = {
                let Some(built) = self.built.as_ref() else {
                    return Diagnostics::error("no_document", "no document loaded");
                };
                let Some(page) = built.pages.get(self.page) else {
                    return Diagnostics::error("page_range", "page index out of range");
                };
                let Some(presenter) = self.presenter.as_ref() else {
                    return Diagnostics::error("no_gpu", "GPU not initialised");
                };
                let scene =
                    SurfacePresenter::build_page_scene(&page.list, page.width_pt, page.height_pt);
                // The page scene is in points; fit it to the surface
                // (device px) with a small margin and centre it.
                let sw = presenter.width() as f32;
                let sh = presenter.height() as f32;
                let pw = page.width_pt.max(1.0);
                let ph = page.height_pt.max(1.0);
                let scale = (sw / pw).min(sh / ph) * 0.95;
                let scale = scale.max(0.0);
                let tx = (sw - pw * scale) * 0.5;
                let ty = (sh - ph * scale) * 0.5;
                (scene, [scale, 0.0, 0.0, scale, tx, ty])
            };

            let Some(presenter) = self.presenter.as_mut() else {
                return Diagnostics::error("no_gpu", "GPU not initialised");
            };
            // Light-grey surround behind the page (page body + border come
            // from `build_page_scene`).
            let bg = Color::rgba(0.898, 0.905, 0.922, 1.0);
            match presenter.present_scenes(&[(&scene, transform)], bg) {
                Ok(()) => Diagnostics::ok(),
                Err(e) => Diagnostics::error("present", &format!("present: {e}")),
            }
        }

        /// Ensure a presenter (hence a wgpu device) exists for headless
        /// readback. Reuses the on-screen presenter if one is bound; else
        /// creates a throwaway 1×1 `OffscreenCanvas` purely for its device —
        /// `render_scene_to_rgba` renders to its own target texture, so the
        /// backing canvas size is irrelevant.
        async fn ensure_presenter(&mut self) -> Result<(), String> {
            if self.presenter.is_some() {
                return Ok(());
            }
            let canvas = web_sys::OffscreenCanvas::new(1, 1)
                .map_err(|e| format!("OffscreenCanvas::new: {e:?}"))?;
            let presenter = SurfacePresenter::new_offscreen(canvas, 1, 1)
                .await
                .map_err(|e| format!("WebGPU init: {e}"))?;
            self.presenter = Some(presenter);
            Ok(())
        }
    }

    /// True when `globalThis.navigator.gpu` is present. Works in both the
    /// window and worker scopes (read via `Reflect`, so no Window-vs-Worker
    /// web-sys feature split).
    fn webgpu_available() -> bool {
        let global = js_sys::global();
        let Ok(nav) = js_sys::Reflect::get(&global, &JsValue::from_str("navigator")) else {
            return false;
        };
        if nav.is_undefined() || nav.is_null() {
            return false;
        }
        match js_sys::Reflect::get(&nav, &JsValue::from_str("gpu")) {
            Ok(gpu) => !gpu.is_undefined() && !gpu.is_null(),
            Err(_) => false,
        }
    }
}

#[cfg(all(target_arch = "wasm32", feature = "gpu"))]
pub use wasm::*;

// Non-wasm builds keep the library buildable — important for
// `cargo check --workspace` on native hosts and for `cargo doc`. The real
// API is only available when built for `wasm32` with `--features gpu`.
#[cfg(not(all(target_arch = "wasm32", feature = "gpu")))]
pub mod native_shim {
    //! Stub surface that makes the crate compile off the wasm/GPU target.

    pub fn is_wasm() -> bool {
        false
    }
}
