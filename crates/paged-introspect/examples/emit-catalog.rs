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

//! Emit the capability catalog as pretty JSON — the committed build-time
//! artifact (`crates/paged-introspect/catalog.json`) that off-engine consumers
//! (the plugin SDK sync, `state`'s catalog ingest, docs) read without booting
//! wasm. Regenerate after any catalog change:
//!
//! ```sh
//! cargo run -p paged-introspect --example emit-catalog > crates/paged-introspect/catalog.json
//! ```
//!
//! The `catalog_json_artifact_is_current` test fails if the committed file
//! drifts from `api_catalog()`.

fn main() {
    let json = serde_json::to_string_pretty(&paged_introspect::api_catalog())
        .expect("serialize the capability catalog");
    println!("{json}");
}
