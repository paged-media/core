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

//! wasm-bindgen surface for the IDML Web Canvas worker.
//!
//! Thin layer over `paged-canvas`. The worker bundle in
//! `apps/canvas/` constructs `CanvasWorker`, then forwards every
//! `MessageEvent` from the main thread through `handle_message`,
//! which returns a JSON-serialisable `WorkerToMain` envelope the
//! worker `postMessage`s back.
//!
//! No render logic lives here — that stays in `paged-canvas` so the
//! Tier 4 path can be exercised headlessly via `cargo test`.
//!
//! The message dispatch itself (parse → per-kind arms → serialise) is
//! cfg-independent and lives in [`dispatch`], compiled on every target
//! and unit-tested natively (`tests/dispatch.rs`). This module is the
//! `#[cfg(wasm32)]` shell: it owns the `js_sys::Date` clock, console
//! logging, and the GPU presenter / Vello scene cache, and forwards
//! parsed messages into [`dispatch::WorkerCore`].

pub mod dispatch;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use crate::dispatch::{CacheEffect, WorkerCore};
    use paged_canvas::{
        snap::SnapLine, CanvasOptions, LoadError, PageId, ProtocolVersion, WorkerToMain,
        WorkerToMainKind, PROTOCOL_VERSION,
    };
    use serde::Serialize;
    use wasm_bindgen::prelude::*;

    /// Return shape of `update_gesture_raw` (Step 5e). Worker parses
    /// this to emit `GestureSnapLines` notifications and to scope its
    /// `markDirty` invalidation. Serialised as JSON because the SAB
    /// hot path bypasses tsify — a flat ad-hoc struct keeps the
    /// boundary thin.
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct GestureRawOutcome {
        page_ids: Vec<PageId>,
        snap_lines: Vec<SnapLine>,
    }

    /// Browser wall-clock in milliseconds — the `Clock` the dispatch
    /// core uses for its `rebuild_ms` instrumentation.
    fn now_ms() -> f64 {
        js_sys::Date::now()
    }

    #[wasm_bindgen(start)]
    pub fn on_start() {
        console_error_panic_hook::set_once();
        web_sys::console::log_1(&"paged-canvas-wasm: init".into());
    }

    /// Worker-side state holder. The JS worker creates one of these
    /// per worker lifetime and forwards `MessageEvent.data` to
    /// `handle_message` after JSON parsing.
    #[wasm_bindgen]
    pub struct CanvasWorker {
        /// Target-agnostic dispatch state: the loaded model, font /
        /// colour-profile registries, and PDF export sessions. The
        /// per-kind message handling lives on this; the wasm shell only
        /// adds the GPU presenter + scene cache below.
        core: WorkerCore,
        #[cfg(feature = "gpu")]
        presenter: Option<paged_gpu::SurfacePresenter>,
        /// Per-page Vello scene cache (sub-phase D). LRU-bounded so
        /// a 500-page document doesn't pin every page's scene at
        /// once. Phase F (Phase 1 polish) added the LRU; the
        /// max-entries budget defaults to `DEFAULT_SCENE_CACHE_PAGES`
        /// and is tuneable via `setSceneCacheBudget`.
        #[cfg(feature = "gpu")]
        scene_cache: SceneCache,
    }

    /// LRU page-scene cache. Keys are page indices into
    /// `model.built().pages`; values are owned `vello::Scene`s built
    /// by `SurfacePresenter::build_page_scene`. Eviction policy: when
    /// the entry count exceeds `max_entries`, drop the least-recently
    /// accessed entry. Touch-on-get keeps the most-recently presented
    /// pages resident.
    #[cfg(feature = "gpu")]
    struct SceneCache {
        entries: std::collections::HashMap<usize, paged_gpu::VelloScene>,
        /// Page indices in LRU order; front = most recent, back =
        /// next to evict. Bounded length matches `entries.len()`.
        order: std::collections::VecDeque<usize>,
        max_entries: usize,
    }

    #[cfg(feature = "gpu")]
    const DEFAULT_SCENE_CACHE_PAGES: usize = 200;

    #[cfg(feature = "gpu")]
    impl SceneCache {
        fn new(max_entries: usize) -> Self {
            Self {
                entries: std::collections::HashMap::new(),
                order: std::collections::VecDeque::new(),
                max_entries: max_entries.max(1),
            }
        }

        fn clear(&mut self) {
            self.entries.clear();
            self.order.clear();
        }

        /// Phase 4 Step 3 — drop the cached scenes for `pages` only.
        /// Other pages keep their cached Vello scene so the next
        /// `presentFrame` skips rebuilding them. Empty `pages` is a
        /// no-op (the caller already knows there's nothing to dirty).
        fn invalidate_pages(&mut self, pages: &[usize]) {
            for &p in pages {
                if self.entries.remove(&p).is_some() {
                    if let Some(pos) = self.order.iter().position(|&k| k == p) {
                        self.order.remove(pos);
                    }
                }
            }
        }

        fn touch(&mut self, key: usize) {
            if let Some(pos) = self.order.iter().position(|&k| k == key) {
                self.order.remove(pos);
            }
            self.order.push_front(key);
        }

        fn get(&mut self, key: usize) -> Option<&paged_gpu::VelloScene> {
            if self.entries.contains_key(&key) {
                self.touch(key);
                self.entries.get(&key)
            } else {
                None
            }
        }

        fn insert(&mut self, key: usize, value: paged_gpu::VelloScene) {
            if self.entries.insert(key, value).is_none() {
                self.touch(key);
            } else {
                self.touch(key);
            }
            while self.entries.len() > self.max_entries {
                if let Some(victim) = self.order.pop_back() {
                    self.entries.remove(&victim);
                } else {
                    break;
                }
            }
        }

        fn len(&self) -> usize {
            self.entries.len()
        }
    }

    #[wasm_bindgen]
    impl CanvasWorker {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                core: WorkerCore::new(),
                #[cfg(feature = "gpu")]
                presenter: None,
                #[cfg(feature = "gpu")]
                scene_cache: SceneCache::new(DEFAULT_SCENE_CACHE_PAGES),
            }
        }

        /// Number of cached page scenes currently resident. Surfaced
        /// for the HUD / DevTools — a developer-facing memory probe.
        #[cfg(feature = "gpu")]
        #[wasm_bindgen(js_name = sceneCacheSize)]
        pub fn scene_cache_size(&self) -> usize {
            self.scene_cache.len()
        }

        /// Override the LRU budget. Useful from a developer console
        /// when measuring memory behaviour.
        #[cfg(feature = "gpu")]
        #[wasm_bindgen(js_name = setSceneCacheBudget)]
        pub fn set_scene_cache_budget(&mut self, max_entries: usize) {
            self.scene_cache.max_entries = max_entries.max(1);
            while self.scene_cache.entries.len() > self.scene_cache.max_entries {
                if let Some(victim) = self.scene_cache.order.pop_back() {
                    self.scene_cache.entries.remove(&victim);
                } else {
                    break;
                }
            }
        }

        /// Protocol version constant; the JS side compares against
        /// its bundled value before sending `LoadDocument`.
        #[wasm_bindgen(getter, js_name = protocolVersion)]
        pub fn protocol_version(&self) -> u32 {
            PROTOCOL_VERSION.0
        }

        /// Worker-internal tile rendering. Bypasses the JSON
        /// `RequestSnapshot` round-trip — for the render loop that
        /// fires every frame, the JSON serialize/parse cost of a
        /// 1024px PNG (~megabyte of `[n, n, n, ...]` text) dominates
        /// the actual rasterization. Returns raw PNG bytes the JS
        /// side feeds straight to `createImageBitmap(blob)`.
        ///
        /// Returns `None` (→ `undefined` on the JS side) if no
        /// document is loaded or the page id is unknown.
        #[wasm_bindgen(js_name = renderTilePng)]
        pub fn render_tile_png(&self, page_id: &str, target_width_px: u32) -> Option<Vec<u8>> {
            let model = self.core.model.as_ref()?;
            let pid = paged_canvas::PageId(page_id.to_string());
            paged_canvas::render_snapshot_png(model, &pid, target_width_px)
                .ok()
                .map(|s| s.png_bytes)
        }

        /// Per-page dimensions for the worker's render loop. Returns
        /// a flat `[page_id_len, ...page_id_utf8, w_pt, h_pt]`-style
        /// blob? No — wasm-bindgen handles `Vec<JsValue>` poorly.
        /// Easier: each call returns one page; iterate by index.
        /// Returns `None` past the end. Tuple is `[page_id, w_pt, h_pt]`
        /// serialised as a JS array.
        #[wasm_bindgen(js_name = pageInfo)]
        pub fn page_info(&self, index: usize) -> Option<js_sys::Array> {
            let model = self.core.model.as_ref()?;
            let page = model.built().pages.get(index)?;
            let arr = js_sys::Array::new_with_length(3);
            arr.set(0, JsValue::from_str(page.id.as_str()));
            arr.set(1, JsValue::from_f64(page.width_pt as f64));
            arr.set(2, JsValue::from_f64(page.height_pt as f64));
            Some(arr)
        }

        /// Direct binary entry point for `loadDocument`. Bypasses the
        /// JSON channel so multi-MB IDMLs don't have to ride as a
        /// 8×-inflated `number[]` array (which on wasm32 trips the
        /// 2 GB `Vec::with_capacity` cap during serde parse — the
        /// megapacks ≥100 MB panic with "capacity overflow" through
        /// the JSON path). Returns a JSON string that the JS side
        /// parses with the same `WorkerToMain` shape `handleMessage`
        /// would produce — `documentLoaded` on success, `loadFailed`
        /// otherwise.
        #[wasm_bindgen(js_name = loadDocumentDirect)]
        pub fn load_document_direct(
            &mut self,
            seq: u32,
            bytes: &[u8],
            font: Option<Vec<u8>>,
            cmyk_icc_profile: Option<Vec<u8>>,
        ) -> String {
            let opts = CanvasOptions {
                fonts: font.map(|b| vec![b]).unwrap_or_default(),
                font_registry: self.core.font_registry.clone(),
                cmyk_icc_profile,
                color_profiles: self.core.color_profiles.clone(),
            };
            let doc_id = format!("doc-{}", seq);
            // u64 because `WorkerToMain.seq` is u64 to match the
            // JSON-channel envelope's existing sequence width.
            let seq_u64 = seq as u64;
            let reply = match paged_canvas::CanvasModel::load(doc_id, bytes, opts) {
                Ok(model) => {
                    let handle = model.handle();
                    self.core.model = Some(model);
                    #[cfg(feature = "gpu")]
                    {
                        self.scene_cache.clear();
                    }
                    WorkerToMain {
                        seq: Some(seq_u64),
                        protocol: PROTOCOL_VERSION,
                        kind: WorkerToMainKind::DocumentLoaded(handle),
                    }
                }
                Err(e) => WorkerToMain {
                    seq: Some(seq_u64),
                    protocol: PROTOCOL_VERSION,
                    kind: WorkerToMainKind::LoadFailed { error: e },
                },
            };
            serde_json::to_string(&reply).unwrap_or_default()
        }

        /// Number of pages in the loaded document, or 0 if no
        /// document is loaded.
        #[wasm_bindgen(js_name = pageCount)]
        pub fn page_count(&self) -> usize {
            self.core
                .model
                .as_ref()
                .map(|m| m.page_count())
                .unwrap_or(0)
        }

        /// Phase 3 — caret geometry for a JSON-encoded
        /// `ContentSelection`. Returns a JSON-encoded `CaretGeometry`
        /// or `null` when the selection's story has no captured
        /// layout. The Overlay calls this on selection change to
        /// position the caret.
        #[wasm_bindgen(js_name = caretGeometryJson)]
        pub fn caret_geometry_json(&self, selection_json: &str) -> Option<String> {
            let sel: paged_canvas::ContentSelection = serde_json::from_str(selection_json).ok()?;
            let model = self.core.model.as_ref()?;
            let geom = paged_canvas::caret_geometry(model.built(), &sel)?;
            serde_json::to_string(&geom).ok()
        }

        /// Phase 3 — selection geometry (rect-per-line) for a
        /// JSON-encoded `ContentSelection`. Returns a JSON array of
        /// `SelectionRect`. Empty array for caret selections.
        #[wasm_bindgen(js_name = selectionGeometryJson)]
        pub fn selection_geometry_json(&self, selection_json: &str) -> Option<String> {
            let sel: paged_canvas::ContentSelection = serde_json::from_str(selection_json).ok()?;
            let model = self.core.model.as_ref()?;
            let rects = paged_canvas::selection_geometry(model.built(), &sel);
            serde_json::to_string(&rects).ok()
        }

        /// Run the Tier 3 resolver against the current model.
        /// Returns the result as a JSON string the JS side can
        /// parse via `JSON.parse`. `null` when no document is loaded.
        /// The worker invokes this after `LoadDocument` succeeds and
        /// posts the parsed result as an unsolicited `resolutionDone`
        /// message to the main thread. Phase 2 — heading anchors and
        /// their assigned page numbers become visible in the UI.
        #[wasm_bindgen(js_name = runResolveJson)]
        pub fn run_resolve_json(&self) -> Option<String> {
            let model = self.core.model.as_ref()?;
            let result = paged_canvas::resolve(
                model.scene(),
                model.built(),
                &paged_canvas::ResolveOptions::default(),
            );
            serde_json::to_string(&result).ok()
        }

        /// S-13 — measure a text run against the loaded document's font
        /// registry. Returns a plain JS object
        /// `{ advance, ascender, descender }` (all in POINTS;
        /// `descender` is negative per the OpenType convention) or
        /// `null` when no document is loaded / the family resolves to no
        /// face (and no default font is registered). `style` is IDML's
        /// `FontStyle` ("Bold", "Italic", …) or omitted. A READ — no
        /// protocol / wire change, no mutation, no undo-log touch. The
        /// face resolution uses the renderer's styled → bare-family →
        /// document-default fallback, so an unknown `family` falls back
        /// to the default face when one is registered.
        #[wasm_bindgen(js_name = measureText)]
        pub fn measure_text(
            &self,
            family: &str,
            style: Option<String>,
            text: &str,
            size_pt: f32,
        ) -> JsValue {
            let Some(model) = self.core.model.as_ref() else {
                return JsValue::NULL;
            };
            let Some(metrics) = model.measure_text(family, style.as_deref(), text, size_pt) else {
                return JsValue::NULL;
            };
            let obj = js_sys::Object::new();
            // Ignore set failures — Reflect::set on a fresh Object never
            // fails in practice; on the off-chance it does, the missing
            // key surfaces as `undefined` on the JS side.
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("advance"),
                &JsValue::from_f64(metrics.advance as f64),
            );
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("ascender"),
                &JsValue::from_f64(metrics.ascender as f64),
            );
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("descender"),
                &JsValue::from_f64(metrics.descender as f64),
            );
            obj.into()
        }

        /// Initialise the WebGPU + Vello surface presenter against
        /// `canvas`. Async because the browser's adapter and device
        /// requests are Promise-based. On success the worker can call
        /// `presentFrame` per render tick; on failure the worker
        /// stays on the CPU snapshot-blit fallback path.
        ///
        /// `width` / `height` are device-pixel dimensions; the JS
        /// caller passes `canvas.width` and `canvas.height` which it
        /// has already sized to `cssWidth * dpr`.
        #[cfg(feature = "gpu")]
        #[wasm_bindgen(js_name = initGpu)]
        pub async fn init_gpu(
            &mut self,
            canvas: web_sys::OffscreenCanvas,
            width: u32,
            height: u32,
        ) -> Result<bool, JsValue> {
            match paged_gpu::SurfacePresenter::new_offscreen(canvas, width, height).await {
                Ok(p) => {
                    self.presenter = Some(p);
                    Ok(true)
                }
                Err(e) => {
                    web_sys::console::warn_1(&format!("initGpu: {e}").into());
                    Ok(false)
                }
            }
        }

        /// Resize the GPU surface. Worker calls this from a
        /// ResizeObserver on the host canvas element.
        #[cfg(feature = "gpu")]
        #[wasm_bindgen(js_name = resizeGpu)]
        pub fn resize_gpu(&mut self, width: u32, height: u32) {
            if let Some(p) = self.presenter.as_mut() {
                p.resize(width, height);
            }
        }

        /// Render the visible pages at the current camera into the
        /// bound surface. Camera operates in CSS pixels; the
        /// presenter applies `dpr` internally as we bake it into the
        /// per-page transforms below.
        ///
        /// Returns `false` if the surface presenter isn't initialised
        /// or no document is loaded — the worker falls back to its
        /// CPU path in that case.
        #[cfg(feature = "gpu")]
        #[wasm_bindgen(js_name = presentFrame)]
        pub fn present_frame(
            &mut self,
            scale: f32,
            tx: f32,
            ty: f32,
            dpr: f32,
        ) -> Result<bool, JsValue> {
            let Some(presenter) = self.presenter.as_mut() else {
                return Ok(false);
            };
            let Some(model) = self.core.model.as_ref() else {
                return Ok(false);
            };

            // Per-page transform: doc-space (pt) → surface-space
            // (device px). The page's origin in doc space is the
            // cumulative y from stacked layout; the camera adds its
            // pan + scale; dpr brings CSS px → device px.
            //
            // Two-pass approach so the scene cache can borrow mut
            // immutably for present_scenes: pass 1 builds + caches,
            // pass 2 reads cache + builds the (scene, transform) list.
            const PAGE_GAP_PT: f32 = 24.0;
            let k = scale * dpr;
            let viewport_w = presenter.width() as f32;
            let viewport_h = presenter.height() as f32;

            // Pass 1: ensure every visible page has a cached scene.
            // Visibility = surface-space bbox intersects [0..vw, 0..vh].
            // Below-visibility-threshold pages don't get touched, so
            // their cache entries become LRU candidates.
            let mut y_pt = 0.0_f32;
            let mut visible_indices: Vec<usize> = Vec::new();
            for (idx, built_page) in model.built().pages.iter().enumerate() {
                let surface_top = ty * dpr + y_pt * k;
                let surface_bottom = surface_top + built_page.height_pt * k;
                let surface_left = tx * dpr;
                let surface_right = surface_left + built_page.width_pt * k;
                let visible = surface_right > 0.0
                    && surface_left < viewport_w
                    && surface_bottom > 0.0
                    && surface_top < viewport_h;
                if visible {
                    visible_indices.push(idx);
                    if self.scene_cache.get(idx).is_none() {
                        let scene = paged_gpu::SurfacePresenter::build_page_scene(
                            &built_page.list,
                            built_page.width_pt,
                            built_page.height_pt,
                        );
                        self.scene_cache.insert(idx, scene);
                    }
                }
                y_pt += built_page.height_pt + PAGE_GAP_PT;
            }

            // Pass 2: build the (scene, transform) list. The cache
            // entries are guaranteed present for every visible page
            // after pass 1.
            let mut pages: Vec<(&paged_gpu::VelloScene, [f32; 6])> = Vec::new();
            let mut y_pt = 0.0_f32;
            for (idx, built_page) in model.built().pages.iter().enumerate() {
                if visible_indices.contains(&idx) {
                    let transform = [k, 0.0, 0.0, k, tx * dpr, ty * dpr + y_pt * k];
                    if let Some(scene) = self.scene_cache.entries.get(&idx) {
                        pages.push((scene, transform));
                    }
                }
                y_pt += built_page.height_pt + PAGE_GAP_PT;
            }

            // Linear-RGB background matching the CPU path (#e5e7eb).
            let bg = paged_compose::Color::rgba(0.831, 0.851, 0.871, 1.0);
            match presenter.present_scenes(&pages, bg) {
                Ok(()) => Ok(true),
                Err(e) => {
                    web_sys::console::warn_1(&format!("presentFrame: {e}").into());
                    Ok(false)
                }
            }
        }

        /// Whether GPU is initialised. The worker checks this each
        /// frame to decide which render path to take. Cheap; just a
        /// pointer-null check.
        #[cfg(feature = "gpu")]
        #[wasm_bindgen(js_name = gpuReady)]
        pub fn gpu_ready(&self) -> bool {
            self.presenter.is_some()
        }

        /// Sub-phase D — render `page_id` to a PNG via the Vello GPU
        /// path (off-surface). Returns `None` if GPU is not
        /// initialised, the page id is unknown, or the underlying
        /// readback fails. The fidelity suite calls this with
        /// `BACKEND=gpu` to test the production hot path; the CPU
        /// path (`renderTilePng`) stays as the deterministic
        /// fallback used in CI.
        #[cfg(feature = "gpu")]
        #[wasm_bindgen(js_name = renderPageVelloPng)]
        pub async fn render_page_vello_png(
            &mut self,
            page_id: String,
            dpi: f32,
        ) -> Option<Vec<u8>> {
            let pid = paged_canvas::PageId(page_id);
            // Build the Vello scene while we still have the model
            // borrow; immediately drop the borrow so the presenter
            // can take `&mut self` next.
            let (scene, width_px, height_px) = {
                let model = self.core.model.as_ref()?;
                let page = model.page(&pid)?;
                let page_scene = paged_gpu::SurfacePresenter::build_page_scene(
                    &page.list,
                    page.width_pt,
                    page.height_pt,
                );
                let scale = (dpi / 72.0) as f64;
                let mut scene = paged_gpu::VelloScene::new();
                scene.append(
                    &page_scene,
                    Some(paged_gpu::vello_kurbo::Affine::scale(scale)),
                );
                let width_px = ((page.width_pt * dpi / 72.0).ceil() as u32).max(1);
                let height_px = ((page.height_pt * dpi / 72.0).ceil() as u32).max(1);
                (scene, width_px, height_px)
            };
            let presenter = self.presenter.as_mut()?;
            presenter
                .render_scene_to_png(&scene, width_px, height_px)
                .await
                .ok()
        }

        /// Handle one main-thread message. Input is the JSON string
        /// the JS side produced via `JSON.stringify(msg)`. Output is
        /// the JSON string the JS side should `JSON.parse` and post
        /// back to the main thread. Returning a string (rather than
        /// a wasm-bindgen-serialised object) keeps the boundary
        /// Step 5d/5e — raw-arg update-gesture entry. The worker drains
        /// the gesture SAB every tick and calls this without going
        /// through `handleMessage`'s JSON envelope. Returns an empty
        /// string on failure (no document loaded or gesture has gone
        /// stale — the worker drops the tick). On success returns a
        /// JSON string with the dirty page set + active snap guides so
        /// the worker can post a `GestureSnapLines` notification and
        /// run its `markDirty` invalidation without re-querying.
        ///
        /// The 64-bit handle arrives split into low/high words because
        /// JS Numbers can't represent the full u64 range cleanly.
        /// `modifier_bits`: bit 0 = shift, bit 1 = alt, bit 2 =
        /// disable_snap (Ctrl, plan-2 §8.4). Matches the SAB layout
        /// in `packages/shell/src/gestures/gesture-sab.ts`.
        #[wasm_bindgen(js_name = updateGestureRaw)]
        pub fn update_gesture_raw(
            &mut self,
            handle_lo: u32,
            handle_hi: u32,
            dx: f32,
            dy: f32,
            modifier_bits: u32,
        ) -> String {
            let Some(model) = self.core.model.as_mut() else {
                return String::new();
            };
            let handle = paged_canvas::gesture::GestureHandle(
                ((handle_hi as u64) << 32) | (handle_lo as u64),
            );
            let modifiers = paged_canvas::gesture::GestureModifiers {
                shift: (modifier_bits & 0b001) != 0,
                alt: (modifier_bits & 0b010) != 0,
                disable_snap: (modifier_bits & 0b100) != 0,
            };
            match model.update_gesture(handle, (dx, dy), modifiers) {
                Ok(result) => {
                    #[cfg(feature = "gpu")]
                    self.scene_cache.clear();
                    let outcome = GestureRawOutcome {
                        page_ids: result.page_ids,
                        snap_lines: result.snap_lines,
                    };
                    serde_json::to_string(&outcome).unwrap_or_default()
                }
                Err(_) => String::new(),
            }
        }

        /// Handle one main-thread message. Input is the JSON string the
        /// JS side produced via `JSON.stringify(msg)`; output is the
        /// JSON string it should `JSON.parse` and post back. Returning a
        /// string (rather than a wasm-bindgen-serialised object) keeps
        /// the boundary simple — no nested serde-wasm-bindgen
        /// conversions, just text in and text out.
        ///
        /// The dispatch itself lives in [`crate::dispatch::WorkerCore`]
        /// (cfg-agnostic, natively tested); the shell supplies the
        /// `js_sys::Date` clock and applies the returned GPU
        /// [`CacheEffect`] to its Vello scene cache.
        #[wasm_bindgen(js_name = handleMessage)]
        pub fn handle_message(&mut self, input: &str) -> String {
            let (reply, effect) = self.core.handle_message(input, &now_ms);
            self.apply_cache_effect(effect);
            reply
        }

        /// Apply the dispatch's GPU scene-cache effect. On a non-gpu
        /// build there is no cache, so this compiles to a no-op (the
        /// effect is computed but ignored — identical to the old shell,
        /// where every `scene_cache` touch was `#[cfg(feature = "gpu")]`).
        #[cfg(feature = "gpu")]
        fn apply_cache_effect(&mut self, effect: CacheEffect) {
            match effect {
                CacheEffect::None => {}
                CacheEffect::ClearAll => self.scene_cache.clear(),
                CacheEffect::InvalidatePages(pages) => self.scene_cache.invalidate_pages(&pages),
            }
        }

        #[cfg(not(feature = "gpu"))]
        fn apply_cache_effect(&mut self, _effect: CacheEffect) {}
    }

    impl Default for CanvasWorker {
        fn default() -> Self {
            Self::new()
        }
    }

    // Convenience for the JS side: hand it the camera SAB byte size
    // so its `new SharedArrayBuffer(N)` call doesn't drift from the
    // Rust contract.
    #[wasm_bindgen(js_name = cameraSabBytes)]
    pub fn camera_sab_bytes() -> usize {
        paged_canvas::CAMERA_SAB_BYTES
    }

    // Full camera SAB layout snapshot — single source of truth for the
    // byte size + offsets the SAB writer/reader use. The TS side
    // (`apps/canvas/src/channel/camera.ts`) reconciles its hardcoded
    // mirror against this struct at worker init.
    #[wasm_bindgen(js_name = cameraSabLayout)]
    pub fn camera_sab_layout() -> paged_canvas::CameraSabLayout {
        paged_canvas::CameraSabLayout::canonical()
    }

    // Gesture SAB byte size — mirrors `cameraSabBytes`. The TS
    // `GestureBuffer` allocator reads this at worker init and asserts
    // its hardcoded mirror matches.
    #[wasm_bindgen(js_name = gestureSabBytes)]
    pub fn gesture_sab_bytes() -> usize {
        paged_canvas::GESTURE_SAB_BYTES
    }

    // Full gesture SAB layout snapshot — single source of truth for the
    // offsets + modifier bit masks the producer/consumer use. The TS
    // side (`packages/shell/src/gestures/gesture-sab.ts`) reconciles
    // its own hardcoded mirror against this struct at worker init and
    // fires a `protocolMismatch` warning on drift.
    #[wasm_bindgen(js_name = gestureSabLayout)]
    pub fn gesture_sab_layout() -> paged_canvas::GestureSabLayout {
        paged_canvas::GestureSabLayout::canonical()
    }

    // Suppress unused-import warning when only the wasm target uses
    // the LoadError import in this module.
    #[allow(dead_code)]
    type _SuppressUnused = (LoadError, ProtocolVersion);
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod native_shim {
    //! Stub surface so the crate compiles on native targets.
    //! The real API lights up on `target_arch = "wasm32"`.

    pub fn is_wasm() -> bool {
        false
    }
}
