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

//! Opening a `.paged` container (ADR 181): the native model part is the
//! model truth when it carries one this engine reads; otherwise the IDML
//! parts are imported, a stale part never failing the load.

use std::io::{Read, Write};

use paged_renderer::container::open_document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

fn idml() -> Vec<u8> {
    paged_gen::write_idml(&paged_gen::samples::geometry::build()).expect("idml")
}

/// The package's parts, plus `pgm` as the native model part.
fn container(idml: &[u8], pgm: &[u8]) -> Vec<u8> {
    let mut src = ZipArchive::new(std::io::Cursor::new(idml)).expect("zip");
    let mut out = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let mut f = src.by_index(i).expect("entry");
        let method = if f.name() == "mimetype" {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        let name = f.name().to_string();
        let mut body = Vec::new();
        f.read_to_end(&mut body).expect("read");
        out.start_file(
            name,
            SimpleFileOptions::default().compression_method(method),
        )
        .expect("start");
        out.write_all(&body).expect("write");
    }
    out.start_file(paged_store::DOCUMENT_PGM_PATH, SimpleFileOptions::default())
        .expect("start");
    out.write_all(pgm).expect("write");
    out.finish().expect("finish").into_inner()
}

#[test]
fn an_idml_package_is_imported() {
    let opened = open_document(&idml()).expect("open");
    assert!(!opened.from_model_part);
    assert!(!opened.document.spreads.is_empty());
}

#[test]
fn a_container_opens_from_its_model_part() {
    let bytes = idml();
    let mut doc = idml_import::import_idml_doc(&bytes).expect("import");
    let spreads = doc.spreads.len();
    assert!(spreads > 1, "the fixture has more than one spread");
    // An edit the IDML parts do not carry: the model part's truth wins.
    doc.spreads.truncate(1);
    let pgm = paged_store::to_bytes(&doc).expect("pgm");
    let opened = open_document(&container(&bytes, &pgm)).expect("open");
    assert!(opened.from_model_part);
    assert_eq!(opened.document.spreads.len(), 1);
}

#[test]
fn a_part_this_engine_cannot_read_falls_back_to_the_idml_parts() {
    let bytes = idml();
    let spreads = idml_import::import_idml_doc(&bytes)
        .expect("import")
        .spreads
        .len();
    let opened = open_document(&container(&bytes, b"{\"not\":\"a model\"}")).expect("open");
    assert!(!opened.from_model_part);
    assert_eq!(opened.document.spreads.len(), spreads);
}
