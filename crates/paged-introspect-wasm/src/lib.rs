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

//! wasm-bindgen bridge for the inspector.
//!
//! Single exported object [`Inspector`]. JS side:
//!
//! ```ts
//! import init, { Inspector, describeCatalog } from 'paged-introspect-wasm';
//! await init();
//! const catalog = JSON.parse(describeCatalog());   // engine capability catalog (no document needed)
//! const insp = new Inspector(idmlBytes);
//! const tree = JSON.parse(insp.tree());            // hierarchical tree
//! const props = JSON.parse(insp.properties(nodeJson));
//! const applied = JSON.parse(insp.apply(opJson));  // AppliedOperationJson
//! const undone = JSON.parse(insp.undo());          // null when stack empty
//! const redone = JSON.parse(insp.redo());          // null when stack empty
//! const png = insp.renderPage(0, 144);             // Uint8Array
//! ```
//!
//! The wire format is JSON-over-strings — same shape as the Rust-side
//! `Operation` / `AppliedOperation` (`Serialize`/`Deserialize` on
//! both ends). Per RETROSPECTIVE.md this isn't the best long-term
//! answer, but it keeps the bridge surface tiny; promoting to typed
//! objects via `serde-wasm-bindgen` is a follow-up.

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::cell::RefCell;
    use std::rc::Rc;

    #[cfg(feature = "render")]
    use paged_introspect::render_page_png;
    use paged_introspect::tree::NodeIdJson;
    use paged_introspect::{build_tree, describe};
    use paged_mutate::{AppliedOperation, NodeId, Operation, Project};
    use paged_scene::Document;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn on_start() {
        console_error_panic_hook::set_once();
        web_sys::console::log_1(&"paged-introspect-wasm: init".into());
    }

    /// The engine capability catalog (host fns, id grammar, the settable
    /// property paths, constraints) as a JSON string — the published
    /// single-source contract every surface projects (ADR 019). Static: needs
    /// no open document. The headless `paged-run describe` emits the same
    /// `paged_introspect::api_catalog()`.
    #[wasm_bindgen(js_name = describeCatalog)]
    pub fn describe_catalog() -> Result<String, JsError> {
        serde_json::to_string(&paged_introspect::api_catalog())
            .map_err(|e| JsError::new(&format!("catalog json: {e}")))
    }

    #[wasm_bindgen]
    pub struct Inspector {
        project: Rc<RefCell<Project>>,
    }

    #[wasm_bindgen]
    impl Inspector {
        /// Open an IDML by bytes.
        #[wasm_bindgen(constructor)]
        pub fn new(idml: &[u8]) -> Result<Inspector, JsError> {
            let document = paged_parse::import_idml_doc(idml)
                .map_err(|e| JsError::new(&format!("open IDML: {e}")))?;
            Ok(Inspector {
                project: Rc::new(RefCell::new(Project::new(document))),
            })
        }

        /// Return the inspector tree as a JSON string.
        pub fn tree(&self) -> Result<String, JsError> {
            let tree = build_tree(self.project.borrow().document());
            serde_json::to_string(&tree).map_err(|e| JsError::new(&format!("tree json: {e}")))
        }

        /// Return property descriptors for a node. `node_json` matches
        /// `NodeIdJson` (e.g. `{"kind":"TextFrame","id":"TextFrame/u1"}`).
        pub fn properties(&self, node_json: &str) -> Result<String, JsError> {
            let node: NodeIdJson = serde_json::from_str(node_json)
                .map_err(|e| JsError::new(&format!("parse node: {e}")))?;
            let descs = describe(self.project.borrow().document(), &NodeId::from(&node));
            serde_json::to_string(&descs).map_err(|e| JsError::new(&format!("props json: {e}")))
        }

        /// Apply an Operation. `op_json` is the wire form of
        /// `paged_mutate::Operation`. Returns the wire form of
        /// `AppliedOperation` on success.
        pub fn apply(&self, op_json: &str) -> Result<String, JsError> {
            let op: Operation = serde_json::from_str(op_json)
                .map_err(|e| JsError::new(&format!("parse op: {e}")))?;
            let applied = self
                .project
                .borrow_mut()
                .apply(op)
                .map_err(|e| JsError::new(&format!("apply: {e}")))?;
            applied_to_json(&applied)
        }

        /// Undo the most recent op. Returns the resulting
        /// `AppliedOperation` (whose `op` is the inverse that just
        /// ran) as JSON, or the literal `"null"` when the undo stack
        /// is empty.
        pub fn undo(&self) -> Result<String, JsError> {
            match self
                .project
                .borrow_mut()
                .undo()
                .map_err(|e| JsError::new(&format!("undo: {e}")))?
            {
                Some(applied) => applied_to_json(&applied),
                None => Ok("null".to_string()),
            }
        }

        /// Redo the most recently undone op. Symmetric to `undo`.
        pub fn redo(&self) -> Result<String, JsError> {
            match self
                .project
                .borrow_mut()
                .redo()
                .map_err(|e| JsError::new(&format!("redo: {e}")))?
            {
                Some(applied) => applied_to_json(&applied),
                None => Ok("null".to_string()),
            }
        }

        /// Render a page as PNG bytes. Requires the `render` feature.
        #[cfg(feature = "render")]
        #[wasm_bindgen(js_name = renderPage)]
        pub fn render_page(&self, page_index: usize, dpi: f32) -> Result<Vec<u8>, JsError> {
            render_page_png(self.project.borrow().document(), page_index, dpi)
                .map_err(|e| JsError::new(&format!("render: {e}")))
        }

        /// Stub for `renderPage` when the `render` feature is off.
        #[cfg(not(feature = "render"))]
        #[wasm_bindgen(js_name = renderPage)]
        pub fn render_page(&self, _page_index: usize, _dpi: f32) -> Result<Vec<u8>, JsError> {
            Err(JsError::new(
                "paged-introspect-wasm built without the `render` feature \
                 — rebuild with `--features render` once paged-renderer compiles",
            ))
        }
    }

    fn applied_to_json(applied: &AppliedOperation) -> Result<String, JsError> {
        serde_json::to_string(applied).map_err(|e| JsError::new(&format!("applied json: {e}")))
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod native_shim {
    //! Stub surface that keeps the crate compiling on native targets.
    //! The real API is only available on wasm32.
    pub fn is_wasm() -> bool {
        false
    }
}
