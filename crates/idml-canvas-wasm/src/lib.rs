//! wasm-bindgen surface for the IDML Web Canvas worker.
//!
//! Thin layer over `idml-canvas`. The worker bundle in
//! `apps/canvas/` constructs `CanvasWorker`, then forwards every
//! `MessageEvent` from the main thread through `handle_message`,
//! which returns a JSON-serialisable `WorkerToMain` envelope the
//! worker `postMessage`s back.
//!
//! No render logic lives here — that stays in `idml-canvas` so the
//! Tier 4 path can be exercised headlessly via `cargo test`.

#[cfg(target_arch = "wasm32")]
mod wasm {
    use idml_canvas::{
        CanvasModel, CanvasOptions, LoadError, MainToWorker, MainToWorkerKind, ProtocolVersion,
        WorkerError, WorkerToMain, WorkerToMainKind, PROTOCOL_VERSION,
    };
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn on_start() {
        console_error_panic_hook::set_once();
        web_sys::console::log_1(&"idml-canvas-wasm: init".into());
    }

    /// Worker-side state holder. The JS worker creates one of these
    /// per worker lifetime and forwards `MessageEvent.data` to
    /// `handle_message` after JSON parsing.
    #[wasm_bindgen]
    pub struct CanvasWorker {
        model: Option<CanvasModel>,
        #[cfg(feature = "gpu")]
        presenter: Option<idml_gpu::SurfacePresenter>,
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
        entries: std::collections::HashMap<usize, idml_gpu::VelloScene>,
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

        fn touch(&mut self, key: usize) {
            if let Some(pos) = self.order.iter().position(|&k| k == key) {
                self.order.remove(pos);
            }
            self.order.push_front(key);
        }

        fn get(&mut self, key: usize) -> Option<&idml_gpu::VelloScene> {
            if self.entries.contains_key(&key) {
                self.touch(key);
                self.entries.get(&key)
            } else {
                None
            }
        }

        fn insert(&mut self, key: usize, value: idml_gpu::VelloScene) {
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
                model: None,
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
            let pid = idml_canvas::PageId(page_id.to_string());
            idml_canvas::render_snapshot_png(model, &pid, target_width_px)
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
            let sel: idml_canvas::ContentSelection =
                serde_json::from_str(selection_json).ok()?;
            let model = self.model.as_ref()?;
            let geom = idml_canvas::caret_geometry(model.built(), &sel)?;
            serde_json::to_string(&geom).ok()
        }

        /// Phase 3 — selection geometry (rect-per-line) for a
        /// JSON-encoded `ContentSelection`. Returns a JSON array of
        /// `SelectionRect`. Empty array for caret selections.
        #[wasm_bindgen(js_name = selectionGeometryJson)]
        pub fn selection_geometry_json(&self, selection_json: &str) -> Option<String> {
            let sel: idml_canvas::ContentSelection =
                serde_json::from_str(selection_json).ok()?;
            let model = self.model.as_ref()?;
            let rects = idml_canvas::selection_geometry(model.built(), &sel);
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
            let result = idml_canvas::resolve(
                model.scene(),
                model.built(),
                &idml_canvas::ResolveOptions::default(),
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
            match idml_gpu::SurfacePresenter::new_offscreen(canvas, width, height).await {
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
                        let scene = idml_gpu::SurfacePresenter::build_page_scene(
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
            let mut pages: Vec<(&idml_gpu::VelloScene, [f32; 6])> = Vec::new();
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
            let bg = idml_compose::Color::rgba(0.831, 0.851, 0.871, 1.0);
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

        /// Handle one main-thread message. Input is the JSON string
        /// the JS side produced via `JSON.stringify(msg)`. Output is
        /// the JSON string the JS side should `JSON.parse` and post
        /// back to the main thread. Returning a string (rather than
        /// a wasm-bindgen-serialised object) keeps the boundary
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
                        cmyk_icc_profile: cmyk_icc_profile.map(|b| b.into_vec()),
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
                    match model.apply_mutation(&m) {
                        Ok(outcome) => {
                            // Phase 3 correctness — text mutations
                            // succeed; invalidate the entire scene
                            // cache (we don't yet track per-page
                            // dirty ranges) and post MutationApplied.
                            #[cfg(feature = "gpu")]
                            {
                                self.scene_cache.clear();
                            }
                            WorkerToMainKind::MutationApplied {
                                client_seq: msg.seq,
                                applied_seq: outcome.applied_seq,
                                page_ids: outcome.page_ids,
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
                    page_id, doc_point, ..
                } => {
                    let result = self
                        .model
                        .as_ref()
                        .map(|m| m.hit_test(&page_id, doc_point))
                        .unwrap_or_default();
                    WorkerToMainKind::HitResult(idml_canvas::HitResult {
                        frame_id: result.frame_id,
                        story_id: result.story_id,
                        offset_within_story: result.offset_within_story,
                        frame_bounds: result.frame_bounds.map(|b| {
                            idml_canvas::channel::FrameBounds {
                                left: b[0],
                                top: b[1],
                                right: b[2],
                                bottom: b[3],
                            }
                        }),
                    })
                }
                MainToWorkerKind::RequestSnapshot {
                    page_id,
                    target_width_px,
                } => {
                    let Some(model) = self.model.as_ref() else {
                        return WorkerToMain {
                            seq,
                            protocol: PROTOCOL_VERSION,
                            kind: WorkerToMainKind::SnapshotFailed {
                                error: idml_canvas::SnapshotError::UnknownPage { page_id },
                            },
                        };
                    };
                    match idml_canvas::render_snapshot_png(model, &page_id, target_width_px) {
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
                    let rects = idml_canvas::selection_geometry(model.built(), &selection);
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
                    let caret = idml_canvas::caret_geometry(model.built(), &selection);
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
                    match model.undo() {
                        Some(outcome) => {
                            #[cfg(feature = "gpu")]
                            {
                                self.scene_cache.clear();
                            }
                            WorkerToMainKind::UndoApplied {
                                undone_seq: outcome.undone_seq,
                                applied_seq: outcome.applied_seq,
                                page_ids: outcome.page_ids,
                            }
                        }
                        None => WorkerToMainKind::MutationFailed {
                            error: WorkerError::NotImplemented {
                                what: "undo log empty".into(),
                            },
                        },
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
                    match model.redo() {
                        Some(outcome) => {
                            #[cfg(feature = "gpu")]
                            {
                                self.scene_cache.clear();
                            }
                            WorkerToMainKind::RedoApplied {
                                redone_seq: outcome.undone_seq,
                                applied_seq: outcome.applied_seq,
                                page_ids: outcome.page_ids,
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
        idml_canvas::CAMERA_SAB_BYTES
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
