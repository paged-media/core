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
//! the mutation layer (`paged_mutate::PropertyPath`) can change. See
//! [`rewrite`] for the per-element inventory and the documented losses.

use std::io::{Cursor, Read, Write};

use paged_scene::Document;

pub mod rewrite;

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

    for spread in &doc.spreads {
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
