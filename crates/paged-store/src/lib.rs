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

/// The native `.paged` model format version. **Bump on any change to the
/// model's serde shape** (e.g. the type renames during the `paged-model`
/// extraction) so an incompatible part is REJECTED — [`from_bytes`] returns
/// `None` and the loader falls back to the IDML import — rather than
/// mis-deserialized. The format is pre-stabilization and churns; this gate is
/// what keeps a stale `.pgm` from silently corrupting a reload (ADR-022 Q2).
///
/// - v1: initial native shape.
/// - v2 (N7): the structured `designmap` moved off `container` up to a
///   top-level `Document.designmap` field, and `Container` lost its
///   `designmap` field — a serde-shape change, so a v1 part is rejected.
pub const PGM_FORMAT_VERSION: u32 = 2;

/// The on-disk envelope: a version tag around the model. Serialized borrowed
/// (no clone) and deserialized owned.
#[derive(serde::Serialize)]
struct PgmRef<'a> {
    format_version: u32,
    model: &'a Document,
}

#[derive(serde::Deserialize)]
struct Pgm {
    format_version: u32,
    model: Document,
}

/// Serialize a [`Document`] to native `.paged` model bytes (no IDML), stamped
/// with [`PGM_FORMAT_VERSION`].
pub fn to_bytes(doc: &Document) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&PgmRef {
        format_version: PGM_FORMAT_VERSION,
        model: doc,
    })
}

/// Reconstruct a [`Document`] from native `.paged` model bytes, with **no
/// `Container::open` / IDML parse**.
///
/// Returns `None` when the part is unparseable OR carries an incompatible
/// [`PGM_FORMAT_VERSION`] — the caller then falls back to the IDML import, so a
/// stale/foreign `.pgm` is never mis-deserialized (ADR-022 Q2). On success,
/// rebuilds the `#[serde(skip)]` derived caches.
pub fn from_bytes(bytes: &[u8]) -> Option<Document> {
    let pgm: Pgm = serde_json::from_slice(bytes).ok()?;
    if pgm.format_version != PGM_FORMAT_VERSION {
        return None;
    }
    let mut doc = pgm.model;
    doc.rebuild_indexes();
    Some(doc)
}
