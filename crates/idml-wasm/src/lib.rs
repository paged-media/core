//! wasm-bindgen surface.
//!
//! Wraps `idml-renderer` behind the TypeScript API described in
//! idea.md §14.1. Native builds expose a plain library target so the
//! crate can still participate in `cargo check --workspace`.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    web_sys::console::log_1(&"idml-wasm: init".into());
}
