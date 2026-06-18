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

//! IDML re-serialization — the save-back foundation (W3.B1).
//!
//! Turns a (possibly mutated) [`paged_scene::Document`] back into a
//! valid IDML package so an edited document can be saved.
//!
//! # Strategy: carry-through fidelity
//!
//! The parser keeps a *subset* of every entry's attributes; most entries
//! (fonts, preferences, tags, metadata, the XML backing store) are not
//! modeled at all. Regenerating those from the model would silently drop
//! everything the parser didn't read. So this writer does NOT regenerate
//! the package from scratch. Instead it copies the original package
//! verbatim and **patches only what the model can faithfully express**:
//!
//! * **Pass-through (byte-identical).** Every entry except the changed
//!   Spreads / Stories is copied straight out of the source ZIP with its
//!   original compressed bytes (via [`zip::write::ZipWriter::raw_copy_file`]),
//!   so `mimetype` stays first + stored and untouched entries round-trip
//!   bit-for-bit.
//! * **Patched (streaming rewrite).** `Spreads/*.xml` and `Stories/*.xml`
//!   are rewritten with a quick-xml reader→writer pass that copies the
//!   original token stream and overwrites only the attributes / text the
//!   model owns (see [`rewrite`]). Unknown attributes, child elements,
//!   `<Properties>`, processing instructions, and comments pass through
//!   untouched. When the rewrite produces bytes identical to the source
//!   (the document wasn't mutated in that entry), the entry is copied
//!   verbatim instead — so an unmutated round-trip is byte-identical
//!   across the *whole* package.
//!
//! # API shape
//!
//! [`write_idml`] takes `(&Document, original_bytes)` rather than reading
//! the source package off `Document` — even though `Document` *does*
//! retain the original entries (`Document.container.entries`). Taking the
//! original bytes explicitly keeps the ZIP container structure (entry
//! order, compression, the stored-mimetype rule, local-header layout)
//! available for a faithful re-zip, which the decompressed entry map
//! alone can't reconstruct. No parse-side change is needed.
//!
//! # What is save-able (patch list)
//!
//! The patch surface is the intersection of (a) attributes the parser
//! round-trips onto the model and (b) the page-item / story properties
//! the mutation layer (`paged_mutate::PropertyPath`) can change. On top
//! of that property-patch foundation, W1.15 adds STRUCTURAL save-back:
//! page-item inserts / removes within a spread, new swatches / gradients
//! / styles injected into the Resources entries (see [`resources`]),
//! table-cell text + style edits, and group-member transforms. C-8 adds
//! NEW-ENTRY emission: a story minted post-parse (InsertTextFrame's
//! `parent_story`, `src: ""`) and a spread minted by `InsertPage` are
//! serialised as full parts and referenced from `designmap.xml` (see
//! [`emit`]). See [`rewrite`] for the per-element inventory and the
//! documented losses (removed PAGES still leave an orphaned entry).

use std::io::{Cursor, Read, Write};

use paged_scene::Document;

mod emit;
mod paged;
pub mod resources;
pub mod rewrite;

pub use paged::{idml_parts_hash, write_paged, MANIFEST_NAME, PAGED_PREFIX};

/// Errors raised while re-serializing a document.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("source package is not a readable ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("i/o while assembling the package: {0}")]
    Io(#[from] std::io::Error),
    #[error("xml rewrite of {entry}: {source}")]
    Rewrite {
        entry: String,
        #[source]
        source: quick_xml::Error,
    },
}

/// Re-serialize `doc` back into an IDML package, carrying through the
/// untouched bytes of `original` and patching only the model-owned
/// attributes of the Spreads / Stories.
///
/// `original` must be the IDML byte stream `doc` was parsed from (or one
/// structurally equivalent to it — same entries, same `Self` ids). The
/// returned `Vec<u8>` is a valid `.idml` package: `mimetype` first +
/// stored, every other source entry preserved, the Spreads / Stories
/// reflecting the current model state.
///
/// An unmutated document round-trips byte-identically. A mutated
/// document differs only in the Spreads / Stories whose model the
/// mutation touched.
pub fn write_idml(doc: &Document, original: &[u8]) -> Result<Vec<u8>, WriteError> {
    let mut src = zip::ZipArchive::new(Cursor::new(original))?;
    let out = Cursor::new(Vec::<u8>::new());
    let mut zip = zip::write::ZipWriter::new(out);

    // Pre-build the patched bodies, keyed by entry path. Only entries
    // whose rewrite differs from the source land here; an entry that
    // rewrites identically is dropped so it takes the verbatim path
    // below (preserving byte-identity + original compression).
    let mut patched: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();

    // C-8 — new-entry emission. A spread / story the model carries but
    // the source archive doesn't (a spread minted by `InsertPage`, a
    // story minted by InsertTextFrame's `parent_story` with `src: ""`)
    // is serialised as a FULL part and appended to the package, then
    // referenced from `designmap.xml`. An unmutated document mints
    // nothing, so none of this fires and the round-trip stays
    // byte-identical.
    let dom_version = doc
        .container
        .designmap
        .dom_version
        .clone()
        .unwrap_or_else(|| "20.0".to_string());
    let mut new_entries: Vec<(String, Vec<u8>)> = Vec::new();
    // `(anchor src, new src)` — the anchor is the nearest PRECEDING
    // spread with a source entry, so the designmap ref (whose order is
    // page order) lands next to its host.
    let mut new_spread_refs: Vec<(Option<String>, String)> = Vec::new();
    let mut new_story_srcs: Vec<String> = Vec::new();

    for (i, spread) in doc.spreads.iter().enumerate() {
        if let Some(orig) = entry_bytes(&mut src, &spread.src)? {
            let new = rewrite::rewrite_spread(&orig, &spread.spread).map_err(|source| {
                WriteError::Rewrite {
                    entry: spread.src.clone(),
                    source,
                }
            })?;
            if new != orig.as_slice() {
                patched.insert(spread.src.clone(), new);
            }
        } else if !spread.src.is_empty() {
            let body = emit::spread_part(&spread.spread, &dom_version).map_err(|source| {
                WriteError::Rewrite {
                    entry: spread.src.clone(),
                    source,
                }
            })?;
            let anchor = doc.spreads[..i]
                .iter()
                .rev()
                .find(|prev| src.by_name(&prev.src).is_ok())
                .map(|prev| prev.src.clone());
            new_spread_refs.push((anchor, spread.src.clone()));
            new_entries.push((spread.src.clone(), body));
        }
    }
    for story in &doc.stories {
        if let Some(orig) = entry_bytes(&mut src, &story.src)? {
            let new = rewrite::rewrite_story(&orig, &story.story).map_err(|source| {
                WriteError::Rewrite {
                    entry: story.src.clone(),
                    source,
                }
            })?;
            if new != orig.as_slice() {
                patched.insert(story.src.clone(), new);
            }
        } else {
            // Minted post-parse (`src: ""`); derive the entry name from
            // the `Self` id (`/` → `_` — `derive_story_id` re-derives the
            // sanitized id from this stem on reopen).
            let entry_src = if story.src.is_empty() {
                emit::story_src_for(&story.self_id)
            } else {
                story.src.clone()
            };
            if src.by_name(&entry_src).is_ok() {
                // Derived name collides with an existing entry — leave
                // that entry alone rather than clobber it.
                continue;
            }
            let body = emit::story_part(
                &emit::sanitize_id(&story.self_id),
                &story.story,
                &dom_version,
            )
            .map_err(|source| WriteError::Rewrite {
                entry: entry_src.clone(),
                source,
            })?;
            new_entries.push((entry_src.clone(), body));
            new_story_srcs.push(entry_src);
        }
    }

    // Reference the new parts: a minimal designmap.xml insertion next to
    // the existing `<idPkg:Spread>` / `<idPkg:Story>` elements. Only
    // documents that minted something get a designmap diff.
    const DESIGNMAP_SRC: &str = "designmap.xml";
    if !(new_spread_refs.is_empty() && new_story_srcs.is_empty()) {
        if let Some(orig) = entry_bytes(&mut src, DESIGNMAP_SRC)? {
            let new = emit::patch_designmap(&orig, &new_spread_refs, &new_story_srcs).map_err(
                |source| WriteError::Rewrite {
                    entry: DESIGNMAP_SRC.to_string(),
                    source,
                },
            )?;
            if new != orig.as_slice() {
                patched.insert(DESIGNMAP_SRC.to_string(), new);
            }
        }
    }

    // W1.15 lane 2 — new resources. Swatches / gradients created by ops
    // are injected into `Resources/Graphic.xml`; new paragraph / character
    // styles into `Resources/Styles.xml`. Both patchers are pure
    // pass-throughs when the model carries nothing the source lacks, so
    // an unmutated round-trip leaves these entries byte-identical (and the
    // entry takes the verbatim copy path below).
    const GRAPHIC_SRC: &str = "Resources/Graphic.xml";
    const STYLES_SRC: &str = "Resources/Styles.xml";
    if let Some(orig) = entry_bytes(&mut src, GRAPHIC_SRC)? {
        let new = resources::patch_graphic(&orig, &doc.palette).map_err(|source| {
            WriteError::Rewrite {
                entry: GRAPHIC_SRC.to_string(),
                source,
            }
        })?;
        if new != orig.as_slice() {
            patched.insert(GRAPHIC_SRC.to_string(), new);
        }
    }
    if let Some(orig) = entry_bytes(&mut src, STYLES_SRC)? {
        let new =
            resources::patch_styles(&orig, &doc.styles).map_err(|source| WriteError::Rewrite {
                entry: STYLES_SRC.to_string(),
                source,
            })?;
        if new != orig.as_slice() {
            patched.insert(STYLES_SRC.to_string(), new);
        }
    }

    // Walk the source archive in its original order. Each entry is
    // either substituted (patched body, re-deflated) or copied verbatim
    // with its already-compressed bytes.
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..src.len() {
        let name = {
            let entry = src.by_index_raw(i)?;
            if entry.is_dir() {
                // Directory entries (rare in IDML) copy through as-is.
                drop(entry);
                let entry = src.by_index_raw(i)?;
                zip.raw_copy_file(entry)?;
                continue;
            }
            entry.name().to_string()
        };

        if let Some(body) = patched.get(&name) {
            zip.start_file(&name, deflated)?;
            zip.write_all(body)?;
        } else {
            let entry = src.by_index_raw(i)?;
            zip.raw_copy_file(entry)?;
        }
    }

    // C-8 — the minted parts are appended after every source entry
    // (entry order within the ZIP is irrelevant to the parser; the
    // designmap drives discovery, and `mimetype` stays first).
    for (name, body) in &new_entries {
        zip.start_file(name.as_str(), deflated)?;
        zip.write_all(body)?;
    }

    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}

/// Read one entry's decompressed bytes out of the source archive.
/// `None` when the manifest names a path the package doesn't actually
/// carry (tolerated: that resource simply isn't patched).
fn entry_bytes<R: Read + std::io::Seek>(
    src: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<Option<Vec<u8>>, WriteError> {
    let mut entry = match src.by_name(path) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests;
