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

//! The `.paged` container writer (file-format.md).
//!
//! A `.paged` file is, at all times, a structurally-valid IDML package:
//! the IDML parts stay canonical (written by [`write_idml`]), and each
//! plugin owns a namespace of extra parts under `paged/<plugin>/<id>/…`
//! that ride alongside as ZIP entries UNREFERENCED by `designmap.xml`
//! (the EPUB/ODF private-parts idiom — InDesign ignores them on open).
//!
//! [`write_paged`] layers two things over the IDML write:
//!   1. it appends the model-held `paged/` parts the source archive does
//!      not already carry (existing `paged/` parts round-trip untouched
//!      via the carry-through writer — see the `paged_namespace_*` test),
//!   2. it (re)writes the top-level `manifest.json` — Paged self-identity,
//!      the wire-protocol version, and a CONTENT HASH of the IDML parts
//!      (the §3.1 data-loss guard: on reopen, if the IDML parts changed
//!      but the `paged/` parts vanished, the file went through InDesign).
//!
//! `manifest.json` is built with `serde_json` (the existing dep) and also
//! carries a `parts` INDEX — a faithful, recomputed-each-save record of every
//! `paged/` part the container holds: its path, the owning plugin (the first
//! namespace segment), its byte length, and a content hash (file-format.md
//! §7/§8). That index is the staleness/integrity substrate: a reader can spot
//! a truncated/corrupted part by its hash, attribute parts to plugins without
//! unzipping, and (with a future per-plugin version passed at write time)
//! decide trust/recompute on open. The object↔page-item bindings layer on the
//! same way in a later phase.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use paged_scene::Document;

use crate::{write_idml, WriteError};

/// The top-level container metadata entry name.
pub const MANIFEST_NAME: &str = "manifest.json";
/// The plugin-parts namespace prefix.
pub const PAGED_PREFIX: &str = "paged/";

/// FNV-1a 64-bit over a byte slice (the same family as
/// `DisplayList::digest` — a fast change-detector, NOT a cryptographic or
/// cross-version-persistent hash; it backs the data-loss guard, which only
/// needs "did the IDML parts change since the last Paged save").
fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Content hash of the IDML parts of a package — every entry that is NOT a
/// `paged/` part, NOT the `manifest.json`, and NOT the `mimetype` magic.
/// Order-independent (entries are folded in sorted order, each prefixed by
/// its name + length) so a re-zip that reorders entries hashes the same.
pub fn idml_parts_hash(package: &[u8]) -> Result<String, WriteError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package))?;
    let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i)?;
        if e.is_dir() {
            continue;
        }
        let name = e.name().to_string();
        if name == MANIFEST_NAME || name == "mimetype" || name.starts_with(PAGED_PREFIX) {
            continue;
        }
        let mut buf = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut buf)?;
        parts.insert(name, buf);
    }
    let mut h = FNV_OFFSET;
    for (name, body) in &parts {
        h = fnv1a(h, name.as_bytes());
        h = fnv1a(h, &(body.len() as u64).to_le_bytes());
        h = fnv1a(h, body);
    }
    Ok(format!("fnv1a64:{h:016x}"))
}

/// The owning plugin of a `paged/<plugin>/…` part path — the first namespace
/// segment after the `paged/` prefix. `""` for a malformed/prefix-less path
/// (defensive; callers only pass `paged/` entries here).
fn plugin_of(path: &str) -> &str {
    path.strip_prefix(PAGED_PREFIX)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
}

/// Build the manifest's `parts` index: one entry per `paged/` part, sorted by
/// path (the `BTreeMap` order), each carrying its owning plugin, byte length,
/// and content hash. A pure description of data that is definitely in the
/// container — the integrity/staleness record, NOT a new write surface.
fn parts_index(final_parts: &BTreeMap<String, Vec<u8>>) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = final_parts
        .iter()
        .map(|(path, body)| {
            serde_json::json!({
                "path": path,
                "plugin": plugin_of(path),
                "bytes": body.len(),
                "hash": format!("fnv1a64:{:016x}", fnv1a(FNV_OFFSET, body)),
            })
        })
        .collect();
    serde_json::Value::Array(arr)
}

/// Read one entry's decompressed bytes from a package, or `None` if absent.
fn read_entry(package: &[u8], name: &str) -> Result<Option<Vec<u8>>, WriteError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package))?;
    let present = zip.file_names().any(|n| n == name);
    if !present {
        return Ok(None);
    }
    let mut e = zip.by_name(name)?;
    let mut buf = Vec::with_capacity(e.size() as usize);
    e.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

/// Build the `manifest.json` body by MERGING the core-owned fields into the
/// EXISTING manifest (when present), so unknown top-level keys and unknown
/// third-party plugin metadata round-trip untouched — the same preserve-
/// unknown principle the carry-through writer applies to the IDML parts.
/// Only the fields THIS build owns (identity + protocol + the data-loss
/// hash + dom version) are (re)written; everything else (e.g. a `plugins`
/// map keyed by other plugins, or a future schema field) is preserved.
///
/// `manifest.json` is the file's "this is also a Paged document" marker:
/// Paged detects its own identity by this part + the `paged/` namespace,
/// never by the mimetype (which stays the Adobe IDML magic so InDesign
/// opens the file). Objects serialise with sorted keys (serde_json's
/// default), so a re-save with the same inputs is byte-stable.
fn build_manifest(
    existing: Option<&[u8]>,
    paged_protocol: u32,
    idml_hash: &str,
    dom_version: &str,
    parts: serde_json::Value,
) -> Vec<u8> {
    let mut obj: serde_json::Map<String, serde_json::Value> = existing
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
        .and_then(|v| match v {
            serde_json::Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    obj.insert("v".to_string(), serde_json::json!(1));
    obj.insert("format".to_string(), serde_json::json!("paged-container"));
    obj.insert(
        "pagedProtocol".to_string(),
        serde_json::json!(paged_protocol),
    );
    obj.insert("idmlPartsHash".to_string(), serde_json::json!(idml_hash));
    obj.insert("domVersion".to_string(), serde_json::json!(dom_version));
    // The `parts` index is core-owned and recomputed each save (it must mirror
    // the actual container contents), so it is REPLACED, not merged — unlike a
    // third party's own `plugins`/`x-*` keys, which round-trip untouched.
    obj.insert("parts".to_string(), parts);
    // Serialising a plain object Value is infallible.
    serde_json::to_vec_pretty(&serde_json::Value::Object(obj)).unwrap_or_else(|_| b"{}".to_vec())
}

/// Write `doc` as a `.paged` container: a valid IDML package (via
/// [`write_idml`]) plus the model-held `paged/` parts and a refreshed
/// `manifest.json`.
///
/// `new_parts` are `paged/<…>` entries the model holds that the source
/// archive does not already carry; they are appended. Any entry whose name
/// collides with a `new_parts` key OR with `manifest.json` is REPLACED (so
/// a re-save overwrites a stale manifest / updated part rather than
/// duplicating it). The `mimetype` entry stays first + stored.
pub fn write_paged(
    doc: &Document,
    original: &[u8],
    new_parts: &BTreeMap<String, Vec<u8>>,
    paged_protocol: u32,
) -> Result<Vec<u8>, WriteError> {
    // 1. The canonical IDML write (carry-through + patch). Existing `paged/`
    //    parts in `original` survive here untouched.
    let idml = write_idml(doc, original)?;

    // 2. The data-loss-guard hash over the IDML parts only, merged into the
    //    EXISTING manifest (carried through `write_idml` from `original`) so
    //    unknown fields + other plugins' metadata survive the rewrite.
    let idml_hash = idml_parts_hash(&idml)?;
    let dom_version = doc
        .designmap
        .dom_version
        .clone()
        .unwrap_or_else(|| "20.0".to_string());

    // The FINAL set of `paged/` parts the output will hold — existing ones
    // carried through `idml` (minus those `new_parts` replaces) plus the new
    // ones — folded into the manifest's content index.
    let mut final_parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    {
        let mut zip = zip::ZipArchive::new(Cursor::new(&idml))?;
        for i in 0..zip.len() {
            let mut e = zip.by_index(i)?;
            if e.is_dir() {
                continue;
            }
            let name = e.name().to_string();
            if !name.starts_with(PAGED_PREFIX) || new_parts.contains_key(&name) {
                continue; // not a plugin part, or about to be replaced by a new one
            }
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf)?;
            final_parts.insert(name, buf);
        }
    }
    for (name, body) in new_parts {
        final_parts.insert(name.clone(), body.clone());
    }

    let existing_manifest = read_entry(&idml, MANIFEST_NAME)?;
    let manifest = build_manifest(
        existing_manifest.as_deref(),
        paged_protocol,
        &idml_hash,
        &dom_version,
        parts_index(&final_parts),
    );

    // 3. Re-zip: copy every entry except those we are (re)writing, then
    //    append the fresh manifest + the new parts.
    let mut replace: std::collections::HashSet<&str> =
        new_parts.keys().map(|s| s.as_str()).collect();
    replace.insert(MANIFEST_NAME);

    let mut src = zip::ZipArchive::new(Cursor::new(&idml))?;
    let out = Cursor::new(Vec::<u8>::new());
    let mut zip = zip::write::ZipWriter::new(out);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for i in 0..src.len() {
        let raw = src.by_index_raw(i)?;
        if raw.is_dir() || replace.contains(raw.name()) {
            continue;
        }
        zip.raw_copy_file(raw)?;
    }
    zip.start_file(MANIFEST_NAME, deflated)?;
    zip.write_all(&manifest)?;
    for (name, body) in new_parts {
        zip.start_file(name.as_str(), deflated)?;
        zip.write_all(body)?;
    }
    Ok(zip.finish()?.into_inner())
}
