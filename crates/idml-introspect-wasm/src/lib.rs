//! wasm-bindgen bridge for the inspector.
//!
//! Single exported object [`Inspector`]. JS side:
//!
//! ```ts
//! import init, { Inspector } from 'idml-introspect-wasm';
//! await init();
//! const insp = new Inspector(idmlBytes);
//! const tree = JSON.parse(insp.tree());        // hierarchical tree
//! const props = JSON.parse(insp.properties(nodeJson));
//! insp.apply(mutationJson);                    // returns MutationResult JSON
//! const png = insp.renderPage(0, 144);         // Uint8Array
//! ```
//!
//! The wire format is JSON-over-strings. Per RETROSPECTIVE.md this
//! isn't the best long-term answer, but it keeps the bridge surface
//! tiny in M0; promoting to typed objects via `serde-wasm-bindgen` is
//! a follow-up once the API surface stabilises.

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::cell::RefCell;
    use std::rc::Rc;

    use idml_introspect::{build_tree, describe, descriptor::PropertyKeyJson};
    #[cfg(feature = "render")]
    use idml_introspect::render_page_png;
    use idml_introspect::tree::NodeIdJson;
    use idml_mutate::{Mutation, NodeId, Project, PropertyKey, PropertyValue};
    use idml_scene::Document;
    use serde::{Deserialize, Serialize};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn on_start() {
        console_error_panic_hook::set_once();
        web_sys::console::log_1(&"idml-introspect-wasm: init".into());
    }

    #[wasm_bindgen]
    pub struct Inspector {
        project: Rc<RefCell<Project>>,
    }

    /// JSON wire form of a mutation. The React app builds this and
    /// hands it across; the Rust side parses and dispatches.
    #[derive(Debug, Deserialize)]
    struct MutationJson {
        node: NodeIdJson,
        property: PropertyKeyJson,
        value: MutationValueJson,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", content = "value", rename_all = "camelCase")]
    enum MutationValueJson {
        Bounds([f32; 4]),
        ColorRef(Option<String>),
    }

    /// JSON form of a `MutationResult` returned to JS.
    #[derive(Debug, Serialize)]
    struct MutationResultJson {
        node: NodeIdJson,
        property: PropertyKeyJson,
        previous: idml_introspect::descriptor::AuthoredValue,
        new: idml_introspect::descriptor::AuthoredValue,
        invalidation: String,
    }

    #[wasm_bindgen]
    impl Inspector {
        /// Open an IDML by bytes.
        #[wasm_bindgen(constructor)]
        pub fn new(idml: &[u8]) -> Result<Inspector, JsError> {
            let document =
                Document::open(idml).map_err(|e| JsError::new(&format!("open IDML: {e}")))?;
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

        /// Apply a mutation. Returns a JSON `MutationResultJson`.
        pub fn apply(&self, mutation_json: &str) -> Result<String, JsError> {
            let m: MutationJson = serde_json::from_str(mutation_json)
                .map_err(|e| JsError::new(&format!("parse mutation: {e}")))?;
            let property = PropertyKey::from(m.property);
            let value = match m.value {
                MutationValueJson::Bounds(b) => PropertyValue::Bounds(b),
                MutationValueJson::ColorRef(c) => PropertyValue::ColorRef(c),
            };
            let mutation = Mutation {
                node: NodeId::from(&m.node),
                property,
                value,
            };
            let result = self
                .project
                .borrow_mut()
                .apply(mutation)
                .map_err(|e| JsError::new(&format!("apply: {e}")))?;
            let invalidation = format!("{:?}", result.invalidation);
            let wire = MutationResultJson {
                node: result.node.into(),
                property: result.property.into(),
                previous: result.previous_value.into(),
                new: result.new_value.into(),
                invalidation,
            };
            serde_json::to_string(&wire).map_err(|e| JsError::new(&format!("result json: {e}")))
        }

        /// Render a page as PNG bytes. Requires the `render` feature.
        #[cfg(feature = "render")]
        #[wasm_bindgen(js_name = renderPage)]
        pub fn render_page(&self, page_index: usize, dpi: f32) -> Result<Vec<u8>, JsError> {
            render_page_png(self.project.borrow().document(), page_index, dpi)
                .map_err(|e| JsError::new(&format!("render: {e}")))
        }

        /// Stub for `renderPage` when the `render` feature is off.
        /// Returns an empty Vec + logs to the console so the React
        /// app sees a recognisable "no render" state rather than a
        /// JS-side TypeError on a missing method.
        #[cfg(not(feature = "render"))]
        #[wasm_bindgen(js_name = renderPage)]
        pub fn render_page(&self, _page_index: usize, _dpi: f32) -> Result<Vec<u8>, JsError> {
            Err(JsError::new(
                "idml-introspect-wasm built without the `render` feature \
                 — rebuild with `--features render` once idml-renderer compiles",
            ))
        }
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
