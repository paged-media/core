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
//! carry-through (`SourceArchive`'s byte blobs) is `#[serde(skip)]`, and the model's
//! derived caches are rebuilt via [`Document::rebuild_indexes`] after
//! deserialize — so a document reconstructs from native bytes with **no
//! `open_source_archive` / IDML parse** (N1, Approach A: the "self-owning model"
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

pub mod package;

/// The native `.paged` model format version. **Bump on any change to the
/// model's serde shape** (e.g. the type renames during the `paged-model`
/// extraction) so an incompatible part is REJECTED — [`from_bytes`] returns
/// `None` and the loader falls back to the IDML import — rather than
/// mis-deserialized. The format is pre-stabilization and churns; this gate is
/// what keeps a stale `.pgm` from silently corrupting a reload (ADR-022 Q2).
///
/// - v1: initial native shape.
/// - v2 (N7.1): the structured `designmap` moved off `container` up to a
///   top-level `Document.designmap` field, and `SourceArchive` lost its
///   `designmap` field — a serde-shape change, so a v1 part is rejected.
/// - v3 (N7.2): `Document.container: SourceArchive` became
///   `Document.source: Option<SourceArchive>`, now fully `#[serde(skip)]` (the
///   mimetype no longer rides in the `.pgm`) — a v2 part is rejected.
/// - v4: a page item's `image_bytes` are no longer a JSON array of integers
///   (one number per byte: 29 photos made a 211 MB part). They are a blob
///   reference into [`BLOB_PREFIX`] parts when written by [`to_parts`], a
///   base64 string when written by [`to_bytes`]. A v3 part is still READ
///   (the adapter accepts the old array), so existing documents keep their
///   images; an older engine rejects a v4 part and falls back to the IDML
///   import, as with every version step.
pub const PGM_FORMAT_VERSION: u32 = 4;

/// The oldest part this build still reads.
const OLDEST_READABLE_PGM: u32 = 3;

/// Container namespace of the model's image blobs: one part per distinct
/// image, named by a content hash, so an unchanged image keeps its name
/// (and its bytes need not be rewritten) across saves.
pub const BLOB_PREFIX: &str = "paged/core/model/blobs/";

/// Image blobs by container path, as [`to_parts`] returns them.
pub type BlobParts = std::collections::BTreeMap<String, Vec<u8>>;

/// The blob name inside a container path under [`BLOB_PREFIX`].
pub fn blob_name_of(path: &str) -> Option<&str> {
    path.strip_prefix(BLOB_PREFIX).filter(|n| !n.is_empty())
}

/// A blob's name: the first 128 bits of its BLAKE3 hash, in hex. Content
/// addressed, so a writer may skip a blob the container already holds.
fn blob_name(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..32].to_string()
}

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
/// with [`PGM_FORMAT_VERSION`]. SELF-CONTAINED: image bytes ride inline as
/// base64. A container writer uses [`to_parts`] instead.
pub fn to_bytes(doc: &Document) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&PgmRef {
        format_version: PGM_FORMAT_VERSION,
        model: doc,
    })
}

/// Serialize a [`Document`] for a CONTAINER: the model part, plus one part
/// per distinct image keyed by its container path under [`BLOB_PREFIX`].
/// The model part holds only the blob names.
pub fn to_parts(doc: &Document) -> Result<(Vec<u8>, BlobParts), serde_json::Error> {
    let (pgm, blobs) = paged_model::image_bytes::collect(blob_name, || to_bytes(doc));
    Ok((
        pgm?,
        blobs
            .into_iter()
            .map(|(name, bytes)| (format!("{BLOB_PREFIX}{name}"), bytes))
            .collect(),
    ))
}

/// Reconstruct a [`Document`] from native `.paged` model bytes, with **no
/// `open_source_archive` / IDML parse**.
///
/// Returns `None` when the part is unparseable OR carries an incompatible
/// [`PGM_FORMAT_VERSION`] — the caller then falls back to the IDML import, so a
/// stale/foreign `.pgm` is never mis-deserialized (ADR-022 Q2). On success,
/// rebuilds the `#[serde(skip)]` derived caches.
pub fn from_bytes(bytes: &[u8]) -> Option<Document> {
    from_parts(bytes, std::collections::HashMap::new())
}

/// [`from_bytes`] for a model part written by [`to_parts`]: `blobs` are the
/// container's image blobs by NAME (see [`blob_name_of`]). A part that
/// references a blob the container does not hold is `None`, like any other
/// part that cannot be reconstructed.
pub fn from_parts(
    bytes: &[u8],
    blobs: std::collections::HashMap<String, Vec<u8>>,
) -> Option<Document> {
    let pgm: Pgm =
        paged_model::image_bytes::resolve(blobs, || serde_json::from_slice(bytes)).ok()?;
    if !(OLDEST_READABLE_PGM..=PGM_FORMAT_VERSION).contains(&pgm.format_version) {
        return None;
    }
    let mut doc = pgm.model;
    doc.rebuild_indexes();
    Some(doc)
}
