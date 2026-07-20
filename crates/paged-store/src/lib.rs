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

//! The native **Paged document codec** — (de)serialize a
//! [`paged_scene::Document`] to/from native `.paged` bytes with **no IDML**.
//!
//! This is the counterpart to the IDML import/export adapter: the adapter
//! converts `.idml` ↔ model, this codec persists the model itself. The raw-IDML
//! carry-through (`Container`'s byte blobs) is `#[serde(skip)]`, and the model's
//! derived caches are rebuilt via [`Document::rebuild_indexes`] after
//! deserialize — so a document reconstructs from native bytes with **no
//! `Container::open` / IDML parse** (N1, Approach A: the "self-owning model"
//! first slice).
//!
//! Format is JSON via `serde_json` for now (inspectable, wasm-clean, matching
//! the `document.pgd` precedent); a binary codec is a deferred optimization.
//! The on-disk shape mirrors today's IDML-derived model structure and **will
//! churn** as the model is reclaimed/renamed — treat pre-stabilization `.pgm`
//! parts as throwaway (version-gate before shipping to real documents).

use paged_scene::Document;

/// Canonical container path of the native model part inside a `.paged`
/// document. `paged/core/` is a core-owned namespace.
pub const DOCUMENT_PGM_PATH: &str = "paged/core/model/document.pgm";

/// Serialize a [`Document`] to native `.paged` model bytes (no IDML).
pub fn to_bytes(doc: &Document) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(doc)
}

/// Reconstruct a [`Document`] from native `.paged` model bytes, with **no
/// `Container::open` / IDML parse**: deserialize the primary fields, then
/// rebuild the `#[serde(skip)]` derived caches.
pub fn from_bytes(bytes: &[u8]) -> Result<Document, serde_json::Error> {
    let mut doc: Document = serde_json::from_slice(bytes)?;
    doc.rebuild_indexes();
    Ok(doc)
}
