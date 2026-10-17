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

//! Opening a document's bytes: an IDML package, or a `.paged` container
//! (an IDML package that may also carry the native model part, ADR-021),
//! the same way for the editor's canvas and the read-only viewer (ADR 181).
//!
//! A container carrying `paged/core/model/document.pgm` is reconstructed
//! from that part with no source parse — it is the model truth, edits not
//! yet written back to the IDML parts included. A part that is absent, or
//! one this engine cannot read (a stale or foreign format version), falls
//! back to importing the IDML parts: a stale part is never a load error
//! (ADR-022 Q2).

use paged_scene::Document;

/// A document opened from its bytes, with the source archive it came from
/// (every container part, for callers that serve or carry them through).
pub struct Opened {
    pub document: Document,
    pub archive: idml_import::SourceArchive,
    /// Whether the document came from the native model part.
    pub from_model_part: bool,
}

/// Open `bytes` as an IDML package or a `.paged` container.
pub fn open_document(bytes: &[u8]) -> Result<Opened, String> {
    let archive = idml_import::open_source_archive(bytes).map_err(|e| e.to_string())?;
    if let Some(mut document) = archive
        .entry(paged_store::DOCUMENT_PGM_PATH)
        .and_then(|pgm| {
            // The model part names its images; their bytes are the
            // container's blob parts (a part written before blobs carries
            // them inline, and needs none).
            paged_store::from_parts(
                &pgm[..],
                archive
                    .entries
                    .iter()
                    .filter_map(|(path, bytes)| {
                        paged_store::blob_name_of(path)
                            .map(|name| (name.to_string(), bytes.to_vec()))
                    })
                    .collect(),
            )
        })
    {
        adopt_style_leading(&mut document, &archive);
        return Ok(Opened {
            document,
            archive,
            from_model_part: true,
        });
    }
    let document = idml_import::import_idml_archive(&archive).map_err(|e| e.to_string())?;
    Ok(Opened {
        document,
        archive,
        from_model_part: false,
    })
}

/// A native model part written before styles modelled their leading
/// (every `.paged` up to 2026-09-05) carries `leading: None` on every
/// style, while the package's own `Resources/Styles.xml` still spells it
/// (`<Properties><Leading type="unit">13</Leading>`). Adopt the
/// stylesheet's leading for every paragraph / character style that has
/// none, so a document reopened from such a part composes the way
/// InDesign composes its IDML — and the way a fresh import would. A part
/// whose styles already carry leading is left alone.
fn adopt_style_leading(doc: &mut Document, container: &idml_import::SourceArchive) {
    let Some(bytes) = container.entry("Resources/Styles.xml") else {
        return;
    };
    let Ok(sheet) = idml_import::styles::parse_stylesheet(bytes) else {
        return;
    };
    for (id, style) in doc.styles.paragraph_styles.iter_mut() {
        if style.leading.is_none() {
            style.leading = sheet.paragraph_styles.get(id).and_then(|s| s.leading);
        }
    }
    for (id, style) in doc.styles.character_styles.iter_mut() {
        if style.leading.is_none() {
            style.leading = sheet.character_styles.get(id).and_then(|s| s.leading);
        }
    }
}
