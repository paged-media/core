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

//! A `.paged` stores each image as a container part of its own; the model
//! part only names it. Checked on the annual-scale workload's 29 photos.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use base64::Engine as _;
use paged_canvas::{CanvasModel, Mutation};

const PROTOCOL: u32 = paged_canvas::channel::PROTOCOL_VERSION.0;

/// Every placed image in the model, by frame id.
fn images(model: &CanvasModel) -> BTreeMap<String, Vec<u8>> {
    model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .filter_map(|r| Some((r.self_id.clone()?, r.image_bytes.clone()?)))
        .collect()
}

fn entries(package: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).expect("zip");
    (0..zip.len())
        .map(|i| {
            let mut e = zip.by_index(i).expect("entry");
            let mut body = Vec::new();
            e.read_to_end(&mut body).expect("read");
            (e.name().to_string(), body)
        })
        .collect()
}

fn blobs(package: &[u8]) -> Vec<String> {
    entries(package)
        .into_keys()
        .filter(|k| paged_store::blob_name_of(k).is_some())
        .collect()
}

fn reload(package: &[u8]) -> CanvasModel {
    CanvasModel::load("reload", package, paged_perf::options()).expect("the .paged loads")
}

#[test]
fn a_saved_document_keeps_its_images_as_parts_and_loads_them_back() {
    let w = paged_perf::build(320);
    let before = images(&w.model);
    assert_eq!(before.len(), 29, "the workload's photos");

    let package = w.model.export_paged(PROTOCOL).expect("export");
    let parts = entries(&package);
    let pgm = &parts[paged_store::DOCUMENT_PGM_PATH];
    assert!(
        pgm.len() < 8 * 1024 * 1024,
        "the model part is the model, not the photos: {} bytes",
        pgm.len()
    );
    assert!(
        !String::from_utf8_lossy(pgm).contains("\"image_bytes\":["),
        "no image is written as an integer array"
    );
    assert_eq!(blobs(&package).len(), 29, "one part per distinct photo");

    assert_eq!(
        images(&reload(&package)),
        before,
        "every image survives the round trip"
    );
}

#[test]
fn saving_again_keeps_the_same_blobs_and_a_replaced_image_swaps_its_blob() {
    let w = paged_perf::build(320);
    let first = w.model.export_paged(PROTOCOL).expect("export");
    let mut model = reload(&first);

    // An untouched re-save: the same blobs, the same images.
    let second = model.export_paged(PROTOCOL).expect("re-export");
    assert_eq!(blobs(&second), blobs(&first));
    assert_eq!(images(&reload(&second)), images(&model));

    // Replace one photo: its old blob leaves the container, the new one
    // arrives, and nothing else moves.
    let frame = w.image_frames[0].clone();
    let fresh = paged_perf::photo(9_999, 64, 48);
    model
        .apply_mutation(&Mutation::ReplaceImageBytes {
            element_id: frame.clone(),
            bytes: Some(fresh.clone().into()),
        })
        .expect("replace the image");
    let third = model.export_paged(PROTOCOL).expect("export after replace");
    let (old, new) = (blobs(&first), blobs(&third));
    assert_eq!(new.len(), old.len(), "still one blob per photo");
    assert_eq!(
        old.iter().filter(|b| !new.contains(b)).count(),
        1,
        "the replaced photo's blob is gone"
    );
    assert_eq!(
        new.iter().filter(|b| !old.contains(b)).count(),
        1,
        "the new photo's blob is there"
    );
    assert_eq!(images(&reload(&third))[&frame], fresh);
}

/// A part written before blobs (format v3: every image an array of
/// integers, nothing under the blob namespace) still loads with its images.
#[test]
fn a_part_written_before_blobs_still_loads_its_images() {
    let w = paged_perf::build(320);
    let before = images(&w.model);
    let package = w.model.export_paged(PROTOCOL).expect("export");

    // The self-contained form, rewritten to the old shape.
    let mut v: serde_json::Value =
        serde_json::from_slice(&paged_store::to_bytes(w.model.scene()).expect("pgm"))
            .expect("json");
    fn to_arrays(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, child) in map.iter_mut() {
                    if k == "image_bytes" {
                        if let Some(s) = child.as_str() {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(s)
                                .expect("base64");
                            *child = serde_json::json!(bytes);
                        }
                    } else {
                        to_arrays(child);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(to_arrays),
            _ => {}
        }
    }
    to_arrays(&mut v);
    v["format_version"] = serde_json::json!(3);
    let legacy_pgm = serde_json::to_vec(&v).expect("legacy pgm");

    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default();
    // `mimetype` first and stored, as the container requires.
    let parts = entries(&package);
    out.start_file("mimetype", stored).unwrap();
    out.write_all(&parts["mimetype"]).unwrap();
    for (name, body) in &parts {
        if name == "mimetype" || paged_store::blob_name_of(name).is_some() {
            continue;
        }
        out.start_file(name.as_str(), deflated).unwrap();
        out.write_all(if name == paged_store::DOCUMENT_PGM_PATH {
            &legacy_pgm
        } else {
            body
        })
        .unwrap();
    }
    let legacy = out.finish().unwrap().into_inner();
    assert!(blobs(&legacy).is_empty());

    assert_eq!(
        images(&reload(&legacy)),
        before,
        "a v3 part keeps its images"
    );
}
