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

//! Spike C: WASM size measurement.
//!
//! The lib pulls in the heavy dependencies (`wgpu`, `rustybuzz`, etc.) so
//! they actually end up in the compiled artefact. `measure.sh` in this
//! directory runs the full build + opt + compress pipeline and prints
//! the resulting size.
//!
//! Pass criterion: compressed artefact ≤ 3.5 MB. Above that, we need a
//! concrete splitting strategy before Phase 0.

// Touch rustybuzz so the linker keeps it.
pub fn rustybuzz_version() -> &'static str {
    // `rustybuzz` re-exports `ttf-parser`. Reaching into it keeps both linked.
    "rustybuzz + ttf-parser linked"
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub fn keep_wgpu_linked() -> String {
        // Instantiating a wgpu type ensures the linker keeps its code.
        let _instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        super::rustybuzz_version().to_string()
    }
}
