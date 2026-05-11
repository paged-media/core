//! wasm-bindgen surface for the editor.
//!
//! This crate is separate from `idml-wasm` so a read-only viewer
//! embed can ship without the editing/undo machinery. The editor
//! shell calls into this module from the main thread; the canvas
//! presenter has to live on whatever thread owns the WebGPU surface
//! (main thread today, since OffscreenCanvas in workers is patchy).
//!
//! M0 surface:
//!
//! ```text
//! open_project(idml: Uint8Array)            -> ProjectHandle
//! handle.epoch / can_undo / can_redo / stats
//! handle.apply_command(json)                -> patchJson
//! handle.undo() / handle.redo()             -> patchJson
//! Editor::new(canvas, w, h)                 -> async Editor
//! editor.attach_project(handle)             -> ()
//! editor.render(zoom, pan_x, pan_y, dpr)    -> ()
//! editor.resize(w, h)                       -> ()
//! ```
//!
//! Native builds expose a stub so the crate participates in
//! `cargo check --workspace`.

#[cfg(target_arch = "wasm32")]
mod wasm {
    use idml_compose::Color as ComposeColor;
    use idml_edit::{hit_test_spread, Command, NodeId, ParaId, Project, StoryId};
    use idml_gpu::{SurfacePresenter, Viewport};
    use idml_renderer::{pipeline, BuiltDocument, PipelineOptions};
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn on_start() {
        console_error_panic_hook::set_once();
        web_sys::console::log_1(&"idml-edit-wasm: init".into());
    }

    /// Browser-side handle to a `Project`. The handle owns the
    /// project; dropping it from JS calls `Drop` on the Rust side.
    /// The `Editor` borrows from this via shared Rc + RefCell so the
    /// presenter can re-render after every command without copying
    /// the project.
    #[wasm_bindgen]
    pub struct ProjectHandle {
        inner: Rc<RefCell<Project>>,
        /// Cached `BuiltDocument` from the most recent build; set
        /// lazily on first render and rebuilt when the epoch advances.
        built: Rc<RefCell<Option<CachedBuild>>>,
    }

    struct CachedBuild {
        epoch: u64,
        doc: BuiltDocument,
    }

    #[wasm_bindgen]
    impl ProjectHandle {
        #[wasm_bindgen(getter)]
        pub fn epoch(&self) -> u64 {
            self.inner.borrow().epoch()
        }

        #[wasm_bindgen(getter)]
        pub fn can_undo(&self) -> bool {
            self.inner.borrow().can_undo()
        }

        #[wasm_bindgen(getter)]
        pub fn can_redo(&self) -> bool {
            self.inner.borrow().can_redo()
        }

        /// Lightweight stats string (JSON). Cheaper than a parse pass.
        #[wasm_bindgen(getter)]
        pub fn stats(&self) -> String {
            let s = self.inner.borrow().stats();
            format!(
                "{{\"spreads\":{},\"stories\":{},\"masterSpreads\":{},\"textFrames\":{}}}",
                s.spreads, s.stories, s.master_spreads, s.text_frames
            )
        }

        /// First page's pt dimensions, useful for fit-to-page on first
        /// render. Returns `[w, h]` as a JS array; empty when the
        /// document has no pages.
        #[wasm_bindgen(getter)]
        pub fn first_page_size_pt(&self) -> js_sys::Array {
            self.ensure_built();
            let arr = js_sys::Array::new();
            if let Some(cached) = self.built.borrow().as_ref() {
                if let Some(p) = cached.doc.pages.first() {
                    arr.push(&JsValue::from_f64(p.width_pt as f64));
                    arr.push(&JsValue::from_f64(p.height_pt as f64));
                }
            }
            arr
        }

        /// Apply a command serialized as JSON. Returns the patch as
        /// JSON. The frontend forwards both shapes without re-parsing
        /// — the bridge's only job is transport.
        #[wasm_bindgen]
        pub fn apply_command(&self, cmd_json: &str) -> Result<String, JsError> {
            let cmd: Command = serde_json::from_str(cmd_json)
                .map_err(|e| JsError::new(&format!("invalid command JSON: {e}")))?;
            let patch = self
                .inner
                .borrow_mut()
                .apply(cmd)
                .map_err(|e| JsError::new(&format!("apply failed: {e}")))?;
            serde_json::to_string(&patch)
                .map_err(|e| JsError::new(&format!("serialize patch: {e}")))
        }

        #[wasm_bindgen]
        pub fn undo(&self) -> Result<String, JsError> {
            let patch = self.inner.borrow_mut().undo();
            serde_json::to_string(&patch)
                .map_err(|e| JsError::new(&format!("serialize patch: {e}")))
        }

        #[wasm_bindgen]
        pub fn redo(&self) -> Result<String, JsError> {
            let patch = self.inner.borrow_mut().redo();
            serde_json::to_string(&patch)
                .map_err(|e| JsError::new(&format!("serialize patch: {e}")))
        }

        /// Serialize the project to native-format bytes (JSON envelope
        /// with the original IDML and the forward command log). The
        /// frontend writes the bytes to disk via the File System
        /// Access API or hands them to OPFS for auto-save.
        #[wasm_bindgen]
        pub fn serialize_native(&self) -> Result<Vec<u8>, JsError> {
            self.inner
                .borrow()
                .serialize_native()
                .map_err(|e| JsError::new(&format!("serialize_native: {e}")))
        }

        /// Story id of the frame's `ParentStory` attribute, or null
        /// if the frame is not a text frame or has no parent story.
        #[wasm_bindgen]
        pub fn parent_story_of_frame(&self, frame_id: &str) -> Option<String> {
            let project = self.inner.borrow();
            project.parent_story_of_frame(frame_id).map(|s| s.0)
        }

        /// Spread index containing `frame_id`, or `-1` when not found.
        #[wasm_bindgen]
        pub fn spread_index_of_frame(&self, frame_id: &str) -> i32 {
            let project = self.inner.borrow();
            project
                .spread_index_of_frame(frame_id)
                .map(|i| i as i32)
                .unwrap_or(-1)
        }

        /// Rectangle frame payload as JSON. Used by the frontend to
        /// implement copy/duplicate without reaching into Rust types.
        /// Returns `null` for non-rectangle ids.
        #[wasm_bindgen]
        pub fn rectangle_payload_json(&self, frame_id: &str) -> String {
            let project = self.inner.borrow();
            let Some(s) = project.rectangle_payload(frame_id) else {
                return "null".into();
            };
            let it = match s.item_transform {
                None => "null".into(),
                Some([a, b, c, d, tx, ty]) => format!("[{a},{b},{c},{d},{tx},{ty}]"),
            };
            format!(
                "{{\"spread_idx\":{},\"bounds\":{{\"top\":{},\"left\":{},\
                 \"bottom\":{},\"right\":{}}},\"item_transform\":{},\
                 \"fill_color\":{},\"stroke_color\":{},\"stroke_weight\":{},\
                 \"applied_object_style\":{},\"image_link\":{}}}",
                s.spread_idx,
                s.bounds.top,
                s.bounds.left,
                s.bounds.bottom,
                s.bounds.right,
                it,
                json_opt_string(s.fill_color.as_deref()),
                json_opt_string(s.stroke_color.as_deref()),
                json_opt_f32(s.stroke_weight),
                json_opt_string(s.applied_object_style.as_deref()),
                json_opt_string(s.image_link.as_deref()),
            )
        }

        /// Concatenated text of paragraph `para` in `story_id`, or
        /// `null` if either is out of range.
        #[wasm_bindgen]
        pub fn paragraph_text(&self, story_id: &str, para: u32) -> Option<String> {
            let project = self.inner.borrow();
            project.paragraph_text(&StoryId(story_id.into()), ParaId(para))
        }

        /// Paragraph count for the given story, or `0` if unknown.
        #[wasm_bindgen]
        pub fn paragraph_count(&self, story_id: &str) -> u32 {
            let project = self.inner.borrow();
            project
                .paragraph_count(&StoryId(story_id.into()))
                .unwrap_or(0) as u32
        }

        /// Paragraph attributes as JSON `{justification, firstLineIndent,
        /// spaceBefore, spaceAfter, paragraphStyle}`. Returns `null`
        /// if the paragraph doesn't exist.
        #[wasm_bindgen]
        pub fn paragraph_attrs_json(&self, story_id: &str, para: u32) -> String {
            let project = self.inner.borrow();
            match project.paragraph_attrs(&StoryId(story_id.into()), ParaId(para)) {
                Some(a) => format!(
                    "{{\"justification\":{},\"firstLineIndent\":{},\
                     \"spaceBefore\":{},\"spaceAfter\":{},\"paragraphStyle\":{}}}",
                    json_opt_string(a.justification.map(|j| j.as_idml())),
                    json_opt_f32(a.first_line_indent),
                    json_opt_f32(a.space_before),
                    json_opt_f32(a.space_after),
                    json_opt_string(a.paragraph_style.as_deref()),
                ),
                None => "null".into(),
            }
        }

        /// Paragraph / Character / Object style lists as JSON arrays
        /// of `[{id, name}]`. Returns `[]` when the document carries
        /// no styles of that flavour.
        #[wasm_bindgen]
        pub fn paragraph_style_list_json(&self) -> String {
            style_list_to_json(self.inner.borrow().paragraph_style_list())
        }
        #[wasm_bindgen]
        pub fn character_style_list_json(&self) -> String {
            style_list_to_json(self.inner.borrow().character_style_list())
        }
        #[wasm_bindgen]
        pub fn object_style_list_json(&self) -> String {
            style_list_to_json(self.inner.borrow().object_style_list())
        }

        /// Layers as JSON `[{id,name,visible,locked}]`.
        #[wasm_bindgen]
        pub fn layer_list_json(&self) -> String {
            let project = self.inner.borrow();
            let mut out = String::from("[");
            for (i, (id, name, vis, lock)) in project.layer_list().iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"id\":{},\"name\":{},\"visible\":{},\"locked\":{}}}",
                    json_string(id),
                    json_string(name),
                    vis,
                    lock,
                ));
            }
            out.push(']');
            out
        }

        /// Master spreads as JSON `[{id,name}]`.
        #[wasm_bindgen]
        pub fn master_spread_list_json(&self) -> String {
            style_list_to_json(self.inner.borrow().master_spread_list())
        }

        /// Pages as JSON `[{id, master}]` (master is a string or null).
        #[wasm_bindgen]
        pub fn page_list_json(&self) -> String {
            let project = self.inner.borrow();
            let mut out = String::from("[");
            for (i, (id, master)) in project.page_list().iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"id\":{},\"master\":{}}}",
                    json_string(id),
                    match master {
                        Some(m) => json_string(m),
                        None => "null".into(),
                    },
                ));
            }
            out.push(']');
            out
        }

        /// Palette swatches as JSON `[{id,name}]`.
        #[wasm_bindgen]
        pub fn swatch_list_json(&self) -> String {
            style_list_to_json(self.inner.borrow().swatch_list())
        }

        /// Snap query during a drag. Returns
        /// `{dx_pt, dy_pt, guides:[{x_a,y_a,x_b,y_b}]}`.
        #[wasm_bindgen]
        pub fn compute_snap_json(
            &self,
            spread_idx: usize,
            x_pt: f32,
            y_pt: f32,
            w_pt: f32,
            h_pt: f32,
            excluded_self_id: Option<String>,
            threshold_pt: f32,
        ) -> String {
            let project = self.inner.borrow();
            let bbox = idml_edit::AabbPt {
                x: x_pt,
                y: y_pt,
                w: w_pt,
                h: h_pt,
            };
            let snap = idml_edit::compute_snap(
                project.document(),
                spread_idx,
                bbox,
                excluded_self_id.as_deref(),
                threshold_pt,
            );
            let mut g = String::from("[");
            for (i, seg) in snap.guides.iter().enumerate() {
                if i > 0 {
                    g.push(',');
                }
                g.push_str(&format!(
                    "{{\"x_a\":{},\"y_a\":{},\"x_b\":{},\"y_b\":{}}}",
                    seg.x_a, seg.y_a, seg.x_b, seg.y_b,
                ));
            }
            g.push(']');
            format!(
                "{{\"dx_pt\":{},\"dy_pt\":{},\"guides\":{}}}",
                snap.delta_x_pt, snap.delta_y_pt, g
            )
        }

        /// First-run attributes as JSON. Used by the Character panel
        /// as a stand-in for the caret's run attrs when no selection
        /// is active.
        #[wasm_bindgen]
        pub fn first_run_attrs_json(&self, story_id: &str, para: u32) -> String {
            let project = self.inner.borrow();
            match project.first_run_attrs(&StoryId(story_id.into()), ParaId(para)) {
                Some(a) => format!(
                    "{{\"font\":{},\"fontStyle\":{},\"pointSize\":{},\
                     \"fillColor\":{},\"tracking\":{},\"underline\":{},\
                     \"strikethru\":{}}}",
                    json_opt_string(a.font.as_deref()),
                    json_opt_string(a.font_style.as_deref()),
                    json_opt_f32(a.point_size),
                    json_opt_string(a.fill_color.as_deref()),
                    json_opt_f32(a.tracking),
                    json_opt_bool(a.underline),
                    json_opt_bool(a.strikethru),
                ),
                None => "null".into(),
            }
        }

        /// Hit-test a click at `(x_pt, y_pt)` in *page-relative* pt
        /// against the spread that contains `page_idx`. Returns the
        /// topmost frame as `{"frame":{"kind":"Frame","id":"..."},
        /// "bbox":{"x":..,"y":..,"w":..,"h":..}}` or `null`.
        ///
        /// The `bbox` is in spread coords; to draw an SVG selection
        /// rectangle on the canvas, subtract the page's spread origin
        /// and apply the viewport.
        #[wasm_bindgen]
        pub fn hit_test(&self, page_idx: usize, x_pt: f32, y_pt: f32) -> String {
            self.ensure_built();
            let built = self.built.borrow();
            let Some(cached) = built.as_ref() else {
                return "null".into();
            };
            let Some(page) = cached.doc.pages.get(page_idx) else {
                return "null".into();
            };
            let (ox, oy) = page.spread_origin;
            // Map page-relative click to spread coords. We don't yet
            // know which spread this page belongs to — pages live in
            // BuiltDocument flattened, but the original spread index
            // is implicit by the page's order. For M1, every page in
            // the seed/sample lives in spread 0. A spread map lookup
            // arrives with multi-spread editing in M3.
            let project = self.inner.borrow();
            let spread_idx = page_to_spread(project.document(), page_idx).unwrap_or(0);
            let hit = hit_test_spread(project.document(), spread_idx, x_pt + ox, y_pt + oy);
            match hit {
                None => "null".into(),
                Some(h) => serde_json::to_string(&HitJson::from(h, ox, oy))
                    .unwrap_or_else(|_| "null".into()),
            }
        }

        /// Get a frame's bbox in *page-relative* pt for the page at
        /// `page_idx`. Returns `null` for unknown frames or frames
        /// not on this page. Used by the editor to draw selection
        /// handles after the user clicks.
        #[wasm_bindgen]
        pub fn frame_bbox_page_pt(&self, page_idx: usize, frame_id: &str) -> String {
            self.ensure_built();
            let built = self.built.borrow();
            let Some(cached) = built.as_ref() else {
                return "null".into();
            };
            let Some(page) = cached.doc.pages.get(page_idx) else {
                return "null".into();
            };
            let (ox, oy) = page.spread_origin;
            let project = self.inner.borrow();
            // Walk every spread/kind looking for the id. Stable but
            // O(N) — same approach as Project::locate_frame.
            for ps in &project.document().spreads {
                if let Some(f) = ps
                    .spread
                    .text_frames
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(frame_id))
                {
                    let bbox = idml_edit::hittest::transformed_bbox(f.bounds, f.item_transform);
                    return serde_json::to_string(&BboxJson {
                        x: bbox.x - ox,
                        y: bbox.y - oy,
                        w: bbox.w,
                        h: bbox.h,
                    })
                    .unwrap_or_else(|_| "null".into());
                }
                if let Some(f) = ps
                    .spread
                    .rectangles
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(frame_id))
                {
                    let bbox = idml_edit::hittest::transformed_bbox(f.bounds, f.item_transform);
                    return serde_json::to_string(&BboxJson {
                        x: bbox.x - ox,
                        y: bbox.y - oy,
                        w: bbox.w,
                        h: bbox.h,
                    })
                    .unwrap_or_else(|_| "null".into());
                }
                if let Some(f) = ps
                    .spread
                    .ovals
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(frame_id))
                {
                    let bbox = idml_edit::hittest::transformed_bbox(f.bounds, f.item_transform);
                    return serde_json::to_string(&BboxJson {
                        x: bbox.x - ox,
                        y: bbox.y - oy,
                        w: bbox.w,
                        h: bbox.h,
                    })
                    .unwrap_or_else(|_| "null".into());
                }
                if let Some(f) = ps
                    .spread
                    .graphic_lines
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(frame_id))
                {
                    let bbox = idml_edit::hittest::transformed_bbox(f.bounds, f.item_transform);
                    return serde_json::to_string(&BboxJson {
                        x: bbox.x - ox,
                        y: bbox.y - oy,
                        w: bbox.w,
                        h: bbox.h,
                    })
                    .unwrap_or_else(|_| "null".into());
                }
                if let Some(f) = ps
                    .spread
                    .polygons
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(frame_id))
                {
                    let bbox = idml_edit::hittest::transformed_bbox(f.bounds, f.item_transform);
                    return serde_json::to_string(&BboxJson {
                        x: bbox.x - ox,
                        y: bbox.y - oy,
                        w: bbox.w,
                        h: bbox.h,
                    })
                    .unwrap_or_else(|_| "null".into());
                }
            }
            "null".into()
        }

        /// (Re)build the cached `BuiltDocument` if the project moved
        /// since the last build. M0 always full-rebuilds; the
        /// incremental pipeline arrives in M1+.
        fn ensure_built(&self) {
            let cur_epoch = self.inner.borrow().epoch();
            let needs_build = match self.built.borrow().as_ref() {
                None => true,
                Some(c) => c.epoch != cur_epoch,
            };
            if !needs_build {
                return;
            }
            let opts = PipelineOptions::default();
            let project = self.inner.borrow();
            match pipeline::build_document(project.document(), &opts) {
                Ok(doc) => {
                    *self.built.borrow_mut() = Some(CachedBuild {
                        epoch: cur_epoch,
                        doc,
                    });
                }
                Err(e) => {
                    web_sys::console::warn_1(&format!("build_document failed: {e}").into());
                }
            }
        }
    }

    fn style_list_to_json(list: Vec<(String, String)>) -> String {
        let mut out = String::from("[");
        for (i, (id, name)) in list.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"id\":{},\"name\":{}}}",
                json_string(id),
                json_string(name)
            ));
        }
        out.push(']');
        out
    }

    fn json_string(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    fn json_opt_string(v: Option<&str>) -> String {
        match v {
            None => "null".into(),
            Some(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        }
    }

    fn json_opt_f32(v: Option<f32>) -> String {
        match v {
            None => "null".into(),
            Some(n) => format!("{n}"),
        }
    }

    fn json_opt_bool(v: Option<bool>) -> String {
        match v {
            None => "null".into(),
            Some(true) => "true".into(),
            Some(false) => "false".into(),
        }
    }

    /// JSON shape for a hit result. Bbox is page-relative pt.
    #[derive(serde::Serialize)]
    struct HitJson {
        frame: NodeId,
        bbox: BboxJson,
    }

    impl HitJson {
        fn from(h: idml_edit::FrameHit, page_origin_x: f32, page_origin_y: f32) -> Self {
            Self {
                frame: h.frame,
                bbox: BboxJson {
                    x: h.bbox.x - page_origin_x,
                    y: h.bbox.y - page_origin_y,
                    w: h.bbox.w,
                    h: h.bbox.h,
                },
            }
        }
    }

    #[derive(serde::Serialize)]
    struct BboxJson {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    }

    /// Map a flat page index back to its source spread index. Walks
    /// the document's spreads and counts pages in order. Returns
    /// `None` if `page_idx` is out of range.
    fn page_to_spread(doc: &idml_scene::Document, page_idx: usize) -> Option<usize> {
        let mut acc = 0usize;
        for (spread_idx, ps) in doc.spreads.iter().enumerate() {
            let n = ps.spread.pages.len();
            if page_idx < acc + n {
                return Some(spread_idx);
            }
            acc += n;
        }
        None
    }

    /// Open an IDML container as an editor `Project`.
    #[wasm_bindgen]
    pub fn open_project(idml: &[u8]) -> Result<ProjectHandle, JsError> {
        let project = Project::open(idml).map_err(|e| JsError::new(&format!("open IDML: {e}")))?;
        Ok(ProjectHandle {
            inner: Rc::new(RefCell::new(project)),
            built: Rc::new(RefCell::new(None)),
        })
    }

    /// Open a previously-saved native project.
    #[wasm_bindgen]
    pub fn open_native_project(bytes: &[u8]) -> Result<ProjectHandle, JsError> {
        let project = Project::deserialize_native(bytes)
            .map_err(|e| JsError::new(&format!("open native project: {e}")))?;
        Ok(ProjectHandle {
            inner: Rc::new(RefCell::new(project)),
            built: Rc::new(RefCell::new(None)),
        })
    }

    /// Editor presentation surface. Owns the wgpu Surface bound to
    /// the canvas; one per editor instance. Constructed async because
    /// the wgpu adapter request returns a Promise on wasm.
    #[wasm_bindgen]
    pub struct Editor {
        presenter: SurfacePresenter,
        attached: Option<ProjectHandle>,
    }

    #[wasm_bindgen]
    impl Editor {
        /// Async constructor. JS calls `await Editor.new(canvas, w, h)`.
        #[wasm_bindgen(js_name = "new")]
        pub async fn new_js(
            canvas: web_sys::HtmlCanvasElement,
            width: u32,
            height: u32,
        ) -> Result<Editor, JsError> {
            let presenter = SurfacePresenter::new(canvas, width, height)
                .await
                .map_err(|e| JsError::new(&format!("surface init: {e}")))?;
            Ok(Editor {
                presenter,
                attached: None,
            })
        }

        /// Attach a project so subsequent `render` calls draw it.
        /// Detaches any previous project (the `ProjectHandle` itself
        /// stays alive on the JS side and can be re-attached later).
        #[wasm_bindgen]
        pub fn attach_project(&mut self, handle: &ProjectHandle) {
            self.attached = Some(ProjectHandle {
                inner: Rc::clone(&handle.inner),
                built: Rc::clone(&handle.built),
            });
        }

        #[wasm_bindgen]
        pub fn detach_project(&mut self) {
            self.attached = None;
        }

        /// Resize the surface (call from a ResizeObserver). Width and
        /// height are device pixels; CSS-pixel sizes get multiplied
        /// by `dpr` on the JS side before being passed in.
        #[wasm_bindgen]
        pub fn resize(&mut self, width: u32, height: u32) {
            self.presenter.resize(width, height);
        }

        /// Render the attached project to the surface at `page_idx`.
        /// `page_idx` of 0 keeps M0/M1 behaviour; multi-page editors
        /// pass the active page so navigation works without
        /// rebuilding the cached `BuiltDocument`.
        #[wasm_bindgen]
        pub fn render(
            &mut self,
            page_idx: u32,
            zoom: f32,
            pan_x: f32,
            pan_y: f32,
            dpr: f32,
        ) -> Result<(), JsError> {
            let viewport = Viewport {
                base_scale: 1.0,
                zoom,
                pan_x,
                pan_y,
                dpr,
            };
            // Disjoint-field borrows so the immutable borrow of
            // `self.attached` doesn't conflict with the mutable
            // borrow of `self.presenter` below.
            let Editor {
                presenter,
                attached,
            } = self;
            if let Some(handle) = attached.as_ref() {
                handle.ensure_built();
                let built = handle.built.borrow();
                if let Some(cached) = built.as_ref() {
                    if let Some(page) = cached.doc.pages.get(page_idx as usize) {
                        return presenter
                            .present(&page.list, viewport, ComposeColor::WHITE)
                            .map_err(|e| JsError::new(&format!("present: {e}")));
                    }
                }
            }
            // Nothing to render → clear to white.
            let empty = idml_compose::DisplayList::default();
            presenter
                .present(&empty, viewport, ComposeColor::WHITE)
                .map_err(|e| JsError::new(&format!("present: {e}")))
        }

        /// Total number of pages in the attached project, or 0.
        #[wasm_bindgen(getter)]
        pub fn page_count(&self) -> u32 {
            let Some(handle) = &self.attached else {
                return 0;
            };
            handle.ensure_built();
            handle
                .built
                .borrow()
                .as_ref()
                .map(|c| c.doc.pages.len() as u32)
                .unwrap_or(0)
        }

        /// Page dimensions in pt for a given index, or empty array.
        #[wasm_bindgen]
        pub fn page_size_pt(&self, page_idx: u32) -> js_sys::Array {
            let arr = js_sys::Array::new();
            let Some(handle) = &self.attached else {
                return arr;
            };
            handle.ensure_built();
            if let Some(cached) = handle.built.borrow().as_ref() {
                if let Some(p) = cached.doc.pages.get(page_idx as usize) {
                    arr.push(&JsValue::from_f64(p.width_pt as f64));
                    arr.push(&JsValue::from_f64(p.height_pt as f64));
                }
            }
            arr
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod native_shim {
    //! Stub surface that makes the crate compile on native targets.
    //! The real API is wasm32-only.
    pub fn is_wasm() -> bool {
        false
    }
}
