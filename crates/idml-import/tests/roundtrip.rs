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

//! Synthetic-IDML round-trip test.
//!
//! Builds a minimal valid IDML container in-memory, hands it to
//! `open_source_archive`, and verifies mimetype + designmap extraction.
//! This is the closest we can get to a corpus-level test without
//! checking in binary fixtures.

use std::io::Write;

use idml_import::open_source_archive;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn build_idml() -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);

    // Per the IDML spec, `mimetype` must be stored (uncompressed) first.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"/>"#,
    )
    .unwrap();

    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:MasterSpread src="MasterSpreads/MasterSpread_ua.xml"/>
  <idPkg:Spread src="Spreads/Spread_u1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_u1.xml", deflated).unwrap();
    zip.write_all(b"<Spread/>").unwrap();

    zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
    zip.write_all(b"<Story/>").unwrap();

    zip.finish().unwrap().into_inner()
}

#[test]
fn opens_synthetic_idml_and_extracts_manifest() {
    let bytes = build_idml();
    let container = open_source_archive(&bytes).expect("valid IDML");
    assert_eq!(
        container.mimetype,
        "application/vnd.adobe.indesign-idml-package"
    );
    // The structured manifest is parsed from the source archive's raw bytes
    // (it no longer lives on SourceArchive — N7).
    let designmap = idml_import::parse_designmap(&container.designmap_raw).expect("designmap");
    assert_eq!(designmap.spreads.len(), 1);
    assert_eq!(designmap.stories.len(), 1);
    assert_eq!(designmap.master_spreads.len(), 1);
    assert_eq!(designmap.spreads[0].src, "Spreads/Spread_u1.xml");
    // Sub-resources are addressable by path.
    assert!(container.entry("Stories/Story_u10.xml").is_some());
    assert!(container.entry("Spreads/Spread_u1.xml").is_some());
}

#[test]
fn rejects_wrong_mimetype() {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/octet-stream").unwrap();
    zip.start_file("designmap.xml", stored).unwrap();
    zip.write_all(b"<Document/>").unwrap();
    let bytes = zip.finish().unwrap().into_inner();

    let err = open_source_archive(&bytes).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not an IDML container"), "got {msg}");
}

#[test]
fn rejects_missing_designmap() {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    let bytes = zip.finish().unwrap().into_inner();

    let err = open_source_archive(&bytes).unwrap_err();
    assert!(err.to_string().contains("designmap.xml"), "got {err}");
}
