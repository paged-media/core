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

#[cfg(target_arch = "wasm32")]
mod wasm {
    use paged_canvas::{
        channel::LayoutCacheStats,
        snap::SnapLine,
        CanvasModel, CanvasOptions, ColorProfileEntry, FontEntry, LoadError, MainToWorker,
        MainToWorkerKind, PageId, ProtocolVersion, WorkerError, WorkerToMain,
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

    /// Editor-ops — snapshot of the built page table (id + size),
    /// diffed across undo/redo so page-mutation reversals carry the
    /// same page-grid refresh fields as `MutationApplied`.
    fn page_table(model: &CanvasModel) -> Vec<(PageId, (f32, f32))> {
        model
            .built()
            .pages
            .iter()
            .map(|p| (p.id.clone(), (p.width_pt, p.height_pt)))
            .collect()
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
        model: Option<CanvasModel>,
        /// Per-family font payloads accumulated via `RegisterFont`.
        /// Survives across `LoadDocument` calls so a Playwright suite
        /// can preload Inter / Poppins / Roboto once per worker, then
        /// step through every pack without re-uploading bytes.
        font_registry: Vec<FontEntry>,
        /// Concept 2 — named ICC profiles registered via
        /// `RegisterColorProfile`. Same lifecycle as the font
        /// registry: survives across `LoadDocument` calls.
        color_profiles: Vec<ColorProfileEntry>,
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

    /// Phase 4 Step 3 — pluck the story id out of a `Mutation` so the
    /// caller can scope GPU cache invalidation. Variants without a
    /// story id (frame moves, page inserts) return None; the caller
    /// falls back to a full cache clear because page-touched-by-frame
    /// hasn't been wired through yet.
    fn story_id_for_mutation(m: &paged_canvas::channel::Mutation) -> Option<String> {
        use paged_canvas::channel::Mutation as M;
        match m {
            M::InsertText { story_id, .. } => Some(story_id.clone()),
            M::DeleteRange { story_id, .. } => Some(story_id.clone()),
            M::ApplyStyle { story_id, .. } => Some(story_id.clone()),
            M::InsertField { story_id, .. } => Some(story_id.clone()),
            _ => None,
        }
    }

    #[wasm_bindgen]
    impl CanvasWorker {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                model: None,
                font_registry: Vec::new(),
                color_profiles: Vec::new(),
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
            let model = self.model.as_ref()?;
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
            let model = self.model.as_ref()?;
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
                font_registry: self.font_registry.clone(),
                cmyk_icc_profile,
                color_profiles: self.color_profiles.clone(),
            };
            let doc_id = format!("doc-{}", seq);
            // u64 because `WorkerToMain.seq` is u64 to match the
            // JSON-channel envelope's existing sequence width.
            let seq_u64 = seq as u64;
            let reply = match CanvasModel::load(doc_id, bytes, opts) {
                Ok(model) => {
                    let handle = model.handle();
                    self.model = Some(model);
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
            self.model.as_ref().map(|m| m.page_count()).unwrap_or(0)
        }

        /// Phase 3 — caret geometry for a JSON-encoded
        /// `ContentSelection`. Returns a JSON-encoded `CaretGeometry`
        /// or `null` when the selection's story has no captured
        /// layout. The Overlay calls this on selection change to
        /// position the caret.
        #[wasm_bindgen(js_name = caretGeometryJson)]
        pub fn caret_geometry_json(&self, selection_json: &str) -> Option<String> {
            let sel: paged_canvas::ContentSelection =
                serde_json::from_str(selection_json).ok()?;
            let model = self.model.as_ref()?;
            let geom = paged_canvas::caret_geometry(model.built(), &sel)?;
            serde_json::to_string(&geom).ok()
        }

        /// Phase 3 — selection geometry (rect-per-line) for a
        /// JSON-encoded `ContentSelection`. Returns a JSON array of
        /// `SelectionRect`. Empty array for caret selections.
        #[wasm_bindgen(js_name = selectionGeometryJson)]
        pub fn selection_geometry_json(&self, selection_json: &str) -> Option<String> {
            let sel: paged_canvas::ContentSelection =
                serde_json::from_str(selection_json).ok()?;
            let model = self.model.as_ref()?;
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
            let model = self.model.as_ref()?;
            let result = paged_canvas::resolve(
                model.scene(),
                model.built(),
                &paged_canvas::ResolveOptions::default(),
            );
            serde_json::to_string(&result).ok()
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
            let Some(model) = self.model.as_ref() else {
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
                let model = self.model.as_ref()?;
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
            let Some(model) = self.model.as_mut() else {
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

        /// simple — no nested serde-wasm-bindgen conversions, just
        /// `Vec<u8>` bytes in and bytes out.
        #[wasm_bindgen(js_name = handleMessage)]
        pub fn handle_message(&mut self, input: &str) -> String {
            let msg: MainToWorker = match serde_json::from_str(input) {
                Ok(m) => m,
                Err(e) => {
                    let err = WorkerToMain {
                        seq: None,
                        protocol: PROTOCOL_VERSION,
                        kind: WorkerToMainKind::Warning {
                            kind: "protocol".into(),
                            details: format!("malformed message: {e}"),
                        },
                    };
                    return serde_json::to_string(&err).unwrap_or_default();
                }
            };
            let reply = self.dispatch(msg);
            serde_json::to_string(&reply).unwrap_or_default()
        }

        fn dispatch(&mut self, msg: MainToWorker) -> WorkerToMain {
            let seq = Some(msg.seq);
            let kind = match msg.kind {
                MainToWorkerKind::Hello => WorkerToMainKind::Ready {
                    protocol: PROTOCOL_VERSION,
                },
                MainToWorkerKind::LoadDocument {
                    bytes,
                    font,
                    cmyk_icc_profile,
                } => {
                    let opts = CanvasOptions {
                        fonts: font.map(|b| vec![b.into_vec()]).unwrap_or_default(),
                        font_registry: self.font_registry.clone(),
                        cmyk_icc_profile: cmyk_icc_profile.map(|b| b.into_vec()),
                        color_profiles: self.color_profiles.clone(),
                    };
                    let doc_id = format!("doc-{}", msg.seq);
                    match CanvasModel::load(doc_id, bytes.as_slice(), opts) {
                        Ok(model) => {
                            let handle = model.handle();
                            self.model = Some(model);
                            // Invalidate the per-page Vello scene
                            // cache — it was keyed to the previous
                            // model's BuiltPages.
                            #[cfg(feature = "gpu")]
                            {
                                self.scene_cache.clear();
                            }
                            WorkerToMainKind::DocumentLoaded(handle)
                        }
                        Err(e) => WorkerToMainKind::LoadFailed { error: e },
                    }
                }
                MainToWorkerKind::Mutate(m) => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::MutationFailed {
                                error: WorkerError::NoDocument,
                            },
                        };
                    };
                    // Phase 4 Step 3 — capture the affected story id
                    // BEFORE applying the mutation; the post-rebuild
                    // story_pages map is the right authority for which
                    // pages the story touches, so we read it after.
                    #[cfg(feature = "gpu")]
                    let affected_story = story_id_for_mutation(&m);
                    let t0 = js_sys::Date::now();
                    match model.apply_mutation(&m) {
                        Ok(outcome) => {
                            // Phase 4 Step 3 — invalidate only the
                            // pages that contain the affected story.
                            // Other pages keep their cached Vello
                            // scenes so `presentFrame` after this
                            // mutation skips a per-page scene rebuild
                            // for every page in the document.
                            #[cfg(feature = "gpu")]
                            {
                                if let Some(sid) = affected_story.as_deref() {
                                    let dirty = model.page_indices_for_story(sid);
                                    if dirty.is_empty() {
                                        // Story has no on-page frames
                                        // (rare — e.g. overflowed
                                        // chain). Fall back to clear.
                                        self.scene_cache.clear();
                                    } else {
                                        self.scene_cache.invalidate_pages(&dirty);
                                    }
                                } else {
                                    self.scene_cache.clear();
                                }
                            }
                            let mut stats: LayoutCacheStats =
                                model.layout_cache_stats().into();
                            stats.rebuild_ms = (js_sys::Date::now() - t0) as f32;
                            // Editor-ops — page-list mutations carry the
                            // refreshed sizes so the editor can rebuild
                            // its page grid without a document reload.
                            let page_sizes_pt = outcome.page_structure_changed.then(|| {
                                model
                                    .built()
                                    .pages
                                    .iter()
                                    .map(|p| (p.width_pt, p.height_pt))
                                    .collect()
                            });
                            WorkerToMainKind::MutationApplied {
                                client_seq: msg.seq,
                                applied_seq: outcome.applied_seq,
                                page_ids: outcome.page_ids,
                                cache_stats: stats,
                                created_id: outcome.created_id,
                                page_structure_changed: outcome.page_structure_changed,
                                page_sizes_pt,
                            }
                        }
                        Err(error) => WorkerToMainKind::MutationFailed { error },
                    }
                }
                MainToWorkerKind::RequestPage { page_id, lod } => {
                    let Some(model) = self.model.as_ref() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::MutationFailed {
                                error: WorkerError::NoDocument,
                            },
                        };
                    };
                    let Some(page) = model.page(&page_id) else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::MutationFailed {
                                error: WorkerError::UnknownPage { page_id },
                            },
                        };
                    };
                    WorkerToMainKind::DisplayListReady {
                        page_id: page.id.clone(),
                        lod,
                        commands: page.list.commands.len(),
                        layout_generation: page.layout_generation,
                        numbering_generation: page.numbering_generation,
                    }
                }
                MainToWorkerKind::HitTest {
                    page_id,
                    doc_point,
                    filter,
                } => {
                    let result = self
                        .model
                        .as_ref()
                        .map(|m| m.hit_test_filtered(&page_id, doc_point, filter))
                        .unwrap_or_default();
                    WorkerToMainKind::HitResult(paged_canvas::HitResult {
                        frame_id: result.frame_id,
                        story_id: result.story_id,
                        offset_within_story: result.offset_within_story,
                        frame_bounds: result.frame_bounds.map(|b| {
                            paged_canvas::channel::FrameBounds {
                                left: b[0],
                                top: b[1],
                                right: b[2],
                                bottom: b[3],
                            }
                        }),
                        element: result.element,
                        bounds: result.bounds,
                        item_transform: result.item_transform,
                        group_chain: result.group_chain,
                    })
                }
                MainToWorkerKind::RequestSnapshot {
                    page_id,
                    target_width_px,
                    dpi,
                } => {
                    let Some(model) = self.model.as_ref() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::SnapshotFailed {
                                error: paged_canvas::SnapshotError::UnknownPage { page_id },
                            },
                        };
                    };
                    let res = match dpi {
                        Some(d) if d > 0.0 => {
                            paged_canvas::render_snapshot_png_at_dpi(model, &page_id, d)
                        }
                        _ => paged_canvas::render_snapshot_png(model, &page_id, target_width_px),
                    };
                    match res {
                        Ok(snap) => WorkerToMainKind::SnapshotReady(snap),
                        Err(error) => WorkerToMainKind::SnapshotFailed { error },
                    }
                }
                MainToWorkerKind::SetSelection { selection } => {
                    if let Some(model) = self.model.as_mut() {
                        model.current_selection = selection;
                        WorkerToMainKind::Stats(model.handle().stats)
                    } else {
                        WorkerToMainKind::MutationFailed {
                            error: WorkerError::NoDocument,
                        }
                    }
                }
                MainToWorkerKind::RequestSelectionGeometry { selection } => {
                    let Some(model) = self.model.as_ref() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::MutationFailed {
                                error: WorkerError::NoDocument,
                            },
                        };
                    };
                    let rects = paged_canvas::selection_geometry(model.built(), &selection);
                    WorkerToMainKind::SelectionGeometry { rects }
                }
                MainToWorkerKind::RequestCaretGeometry { selection } => {
                    let Some(model) = self.model.as_ref() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::MutationFailed {
                                error: WorkerError::NoDocument,
                            },
                        };
                    };
                    let caret = paged_canvas::caret_geometry(model.built(), &selection);
                    WorkerToMainKind::CaretGeometry { caret }
                }
                MainToWorkerKind::Undo => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::MutationFailed {
                                error: WorkerError::NoDocument,
                            },
                        };
                    };
                    let t0 = js_sys::Date::now();
                    // Editor-ops — diff the built page table across the
                    // undo so page-mutation undos refresh the editor's
                    // page grid (same contract as MutationApplied).
                    let pages_before = page_table(model);
                    match model.undo() {
                        Some(outcome) => {
                            #[cfg(feature = "gpu")]
                            {
                                if let Some(sid) = outcome.affected_story_id.as_deref() {
                                    let dirty = model.page_indices_for_story(sid);
                                    if dirty.is_empty() {
                                        self.scene_cache.clear();
                                    } else {
                                        self.scene_cache.invalidate_pages(&dirty);
                                    }
                                } else {
                                    self.scene_cache.clear();
                                }
                            }
                            let mut stats: LayoutCacheStats =
                                model.layout_cache_stats().into();
                            stats.rebuild_ms = (js_sys::Date::now() - t0) as f32;
                            let pages_after = page_table(model);
                            let page_structure_changed = pages_before != pages_after;
                            WorkerToMainKind::UndoApplied {
                                undone_seq: outcome.undone_seq,
                                applied_seq: outcome.applied_seq,
                                page_ids: outcome.page_ids,
                                cache_stats: stats,
                                page_structure_changed,
                                page_sizes_pt: page_structure_changed
                                    .then(|| pages_after.into_iter().map(|p| p.1).collect()),
                            }
                        }
                        None => WorkerToMainKind::MutationFailed {
                            error: WorkerError::NotImplemented {
                                what: "undo log empty".into(),
                            },
                        },
                    }
                }
                MainToWorkerKind::RegisterFont {
                    family,
                    style,
                    bytes,
                } => {
                    self.font_registry.push(FontEntry {
                        family: family.clone(),
                        style,
                        bytes: bytes.into_vec(),
                    });
                    WorkerToMainKind::FontRegistered { family }
                }
                MainToWorkerKind::ClearFontRegistry => {
                    self.font_registry.clear();
                    WorkerToMainKind::FontRegistryCleared
                }
                MainToWorkerKind::RegisterColorProfile { name, bytes } => {
                    let bytes = bytes.into_vec();
                    self.color_profiles.push(ColorProfileEntry {
                        name: name.clone(),
                        bytes: bytes.clone(),
                    });
                    // Keep the LIVE model's registry in sync so a
                    // profile registered after load is immediately
                    // resolvable by SetColorSettings (the worker
                    // copy seeds future loads).
                    if let Some(model) = self.model.as_mut() {
                        model.register_color_profile(name.clone(), bytes);
                    }
                    WorkerToMainKind::ColorProfileRegistered { name }
                }
                MainToWorkerKind::SetElementSelection { ids, mode } => {
                    if let Some(model) = self.model.as_mut() {
                        model.element_selection.apply_mode(&ids, mode);
                        WorkerToMainKind::ElementSelectionApplied {
                            ids: model.element_selection.ids.clone(),
                        }
                    } else {
                        WorkerToMainKind::MutationFailed {
                            error: WorkerError::NoDocument,
                        }
                    }
                }
                MainToWorkerKind::RequestMarqueeHits { page_id, rect } => {
                    let ids = self
                        .model
                        .as_ref()
                        .map(|m| m.marquee_hits(&page_id, rect))
                        .unwrap_or_default();
                    WorkerToMainKind::MarqueeHits { ids }
                }
                MainToWorkerKind::RequestElementGeometry { ids } => {
                    let items = self
                        .model
                        .as_ref()
                        .map(|m| m.element_geometry(&ids))
                        .unwrap_or_default();
                    WorkerToMainKind::ElementGeometry { items }
                }
                MainToWorkerKind::RequestGroupLeaves { group_id } => {
                    let ids = self
                        .model
                        .as_ref()
                        .map(|m| m.group_leaves(&group_id))
                        .unwrap_or_default();
                    WorkerToMainKind::GroupLeaves { ids }
                }
                MainToWorkerKind::RequestPathAnchors { id } => {
                    let result = self.model.as_ref().and_then(|m| m.path_anchors(&id));
                    WorkerToMainKind::PathAnchors { result }
                }
                MainToWorkerKind::RequestLayers => {
                    let items = self
                        .model
                        .as_ref()
                        .map(|m| m.layers())
                        .unwrap_or_default();
                    WorkerToMainKind::Layers { items }
                }
                MainToWorkerKind::RequestCollection { name } => {
                    let items = self
                        .model
                        .as_ref()
                        .map(|m| m.collection(name))
                        .unwrap_or(serde_json::Value::Array(Vec::new()));
                    WorkerToMainKind::CollectionReply { name, items }
                }
                MainToWorkerKind::RequestDocumentMeta => {
                    let meta = self
                        .model
                        .as_ref()
                        .map(|m| m.document_meta())
                        .unwrap_or(paged_canvas::channel::DocumentMeta {
                            page_count: 0,
                            active_page: None,
                            units: String::new(),
                            color_mode: String::new(),
                            document_name: String::new(),
                            dirty: false,
                            default_fill_color: None,
                            default_stroke_color: None,
                            default_stroke_weight: None,
                            cmyk_profile_name: None,
                            rgb_policy: None,
                            rendering_intent: None,
                            black_point_compensation: None,
                            proof_profile_name: None,
                            proof_simulate_paper_white: None,
                            use_standard_lab_for_spots: None,
                        });
                    WorkerToMainKind::DocumentMetaReply { meta }
                }
                MainToWorkerKind::RequestColorPreview { swatch_id } => {
                    let result = self
                        .model
                        .as_ref()
                        .and_then(|m| m.color_preview(&swatch_id));
                    WorkerToMainKind::ColorPreviewReply { result }
                }
                MainToWorkerKind::ExportSwatchLibrary { group_id } => match self.model.as_ref() {
                    Some(m) => WorkerToMainKind::SwatchLibraryExported {
                        ase_bytes: m.export_ase(group_id.as_deref()).into(),
                    },
                    None => WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    },
                },
                MainToWorkerKind::RequestGradientDetail { gradient_id } => {
                    let result = self
                        .model
                        .as_ref()
                        .and_then(|m| m.gradient_detail(&gradient_id));
                    WorkerToMainKind::GradientDetailReply { result }
                }
                MainToWorkerKind::RequestColorCompute {
                    space,
                    value,
                    tint,
                    model,
                    alternate_space,
                    alternate_value,
                } => match self.model.as_ref() {
                    Some(m) => {
                        let (rgb_hex, cmyk, out_of_gamut) = m.color_compute(
                            &space,
                            &value,
                            tint,
                            model.as_deref(),
                            alternate_space.as_deref(),
                            alternate_value.as_deref(),
                        );
                        WorkerToMainKind::ColorComputeReply {
                            rgb_hex,
                            cmyk,
                            out_of_gamut,
                        }
                    }
                    None => WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    },
                },
                MainToWorkerKind::RequestElementProperties { id } => {
                    let result = self
                        .model
                        .as_ref()
                        .and_then(|m| m.element_properties(&id));
                    WorkerToMainKind::ElementProperties { result }
                }
                MainToWorkerKind::RequestSceneTree => {
                    let roots = self
                        .model
                        .as_ref()
                        .map(|m| m.scene_tree())
                        .unwrap_or_default();
                    WorkerToMainKind::SceneTree { roots }
                }
                MainToWorkerKind::ExecuteScript { source } => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::ScriptResult {
                                output: Vec::new(),
                                error: Some("no document loaded".to_string()),
                            },
                        };
                    };
                    let result = paged_script::execute_script(model, &source);
                    WorkerToMainKind::ScriptResult {
                        output: result.output,
                        error: result.error,
                    }
                }
                MainToWorkerKind::BeginGesture {
                    nodes,
                    gesture,
                    anchor,
                    camera_scale,
                } => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::GestureFailed {
                                error: paged_canvas::channel::GestureFailure::NoDocument,
                            },
                        };
                    };
                    match model.begin_gesture_with_scale(nodes, gesture, anchor, camera_scale) {
                        Ok(handle) => WorkerToMainKind::GestureBegun { handle },
                        Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                    }
                }
                MainToWorkerKind::UpdateGesture {
                    handle,
                    delta,
                    modifiers,
                } => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::GestureFailed {
                                error: paged_canvas::channel::GestureFailure::NoDocument,
                            },
                        };
                    };
                    match model.update_gesture(handle, delta, modifiers) {
                        Ok(result) => {
                            // Phase B v1 — clear the GPU scene cache
                            // wholesale on every update. Per-page
                            // invalidation is a Phase B v2 perf knob
                            // once the rebuild path stops dominating.
                            #[cfg(feature = "gpu")]
                            self.scene_cache.clear();
                            WorkerToMainKind::GestureUpdated {
                                handle,
                                page_ids: result.page_ids,
                                snap_lines: result.snap_lines,
                            }
                        }
                        Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                    }
                }
                MainToWorkerKind::CommitGesture { handle } => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::GestureFailed {
                                error: paged_canvas::channel::GestureFailure::NoDocument,
                            },
                        };
                    };
                    let t0 = js_sys::Date::now();
                    match model.commit_gesture(handle) {
                        Ok(outcome) => {
                            #[cfg(feature = "gpu")]
                            self.scene_cache.clear();
                            let mut stats: LayoutCacheStats =
                                model.layout_cache_stats().into();
                            stats.rebuild_ms = (js_sys::Date::now() - t0) as f32;
                            WorkerToMainKind::GestureCommitted {
                                handle,
                                applied_seq: outcome.applied_seq,
                                page_ids: outcome.page_ids,
                                cache_stats: stats,
                            }
                        }
                        Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                    }
                }
                MainToWorkerKind::CancelGesture { handle } => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::GestureFailed {
                                error: paged_canvas::channel::GestureFailure::NoDocument,
                            },
                        };
                    };
                    match model.cancel_gesture(handle) {
                        Ok(page_ids) => {
                            #[cfg(feature = "gpu")]
                            self.scene_cache.clear();
                            WorkerToMainKind::GestureCancelled { handle, page_ids }
                        }
                        Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                    }
                }
                MainToWorkerKind::Redo => {
                    let Some(model) = self.model.as_mut() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::MutationFailed {
                                error: WorkerError::NoDocument,
                            },
                        };
                    };
                    let t0 = js_sys::Date::now();
                    // Editor-ops — page-table diff, same as Undo.
                    let pages_before = page_table(model);
                    match model.redo() {
                        Some(outcome) => {
                            #[cfg(feature = "gpu")]
                            {
                                if let Some(sid) = outcome.affected_story_id.as_deref() {
                                    let dirty = model.page_indices_for_story(sid);
                                    if dirty.is_empty() {
                                        self.scene_cache.clear();
                                    } else {
                                        self.scene_cache.invalidate_pages(&dirty);
                                    }
                                } else {
                                    self.scene_cache.clear();
                                }
                            }
                            let mut stats: LayoutCacheStats =
                                model.layout_cache_stats().into();
                            stats.rebuild_ms = (js_sys::Date::now() - t0) as f32;
                            let pages_after = page_table(model);
                            let page_structure_changed = pages_before != pages_after;
                            WorkerToMainKind::RedoApplied {
                                redone_seq: outcome.undone_seq,
                                applied_seq: outcome.applied_seq,
                                page_ids: outcome.page_ids,
                                cache_stats: stats,
                                page_structure_changed,
                                page_sizes_pt: page_structure_changed
                                    .then(|| pages_after.into_iter().map(|p| p.1).collect()),
                            }
                        }
                        None => WorkerToMainKind::MutationFailed {
                            error: WorkerError::NotImplemented {
                                what: "redo log empty".into(),
                            },
                        },
                    }
                }
            };
            WorkerToMain {
                seq,
                protocol: PROTOCOL_VERSION,
                kind,
            }
        }
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
