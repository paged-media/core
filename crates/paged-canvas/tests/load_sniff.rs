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

//! N3 — the load-time sniff. A `.paged` carrying the native model part
//! (`document.pgm`) reconstructs the model from it with no source parse;
//! plain source packages still take the parse path unchanged.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_renderer::pipeline::{self, PipelineOptions};
use paged_store::DOCUMENT_PGM_PATH;

fn protocol() -> u32 {
    paged_canvas::channel::PROTOCOL_VERSION.0
}

/// Per-page display-list digests of a model's render.
fn digests(model: &CanvasModel) -> Vec<u64> {
    let built = pipeline::build_document(model.scene(), &PipelineOptions::default())
        .expect("build document");
    built.pages.iter().map(|p| p.list.digest()).collect()
}

/// Return a copy of a `.paged`/`.idml` ZIP with an extra entry added at `path`.
fn inject_part(zip_bytes: &[u8], path: &str, bytes: &[u8]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut src = zip::ZipArchive::new(Cursor::new(zip_bytes)).expect("open zip");
    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let mut f = src.by_index(i).expect("read entry");
        let name = f.name().to_string();
        let method = if name == "mimetype" {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let opts = zip::write::SimpleFileOptions::default().compression_method(method);
        zw.start_file(name, opts).expect("copy entry");
        std::io::copy(&mut f, &mut zw).expect("copy bytes");
    }
    zw.start_file(path, zip::write::SimpleFileOptions::default())
        .expect("add part");
    zw.write_all(bytes).expect("write part");
    zw.finish().expect("finish zip").into_inner()
}

/// The load path must prefer the native model part over the source parts. We
/// build a `.paged` whose SOURCE parts are fixture B's but whose native part is
/// fixture A's, then assert it loads as A.
#[test]
fn load_prefers_the_native_model_part_over_the_source_parts() {
    let idml_a =
        paged_gen::write_idml(&paged_gen::samples::geometry::build()).expect("gen fixture a");
    let idml_b =
        paged_gen::write_idml(&paged_gen::samples::strokes_fills::build()).expect("gen fixture b");

    // Fixture A's native model bytes.
    let model_a = CanvasModel::load("a", &idml_a, CanvasOptions::default()).expect("load a");
    let pgm_a = paged_store::to_bytes(model_a.scene()).expect("serialize a");

    // A container that is B's source parts + A's native model part, injected
    // directly into the ZIP — NOT via export, which would auto-embed B's own
    // fresh model part (N4) and defeat the divergence.
    let hybrid_paged = inject_part(&idml_b, DOCUMENT_PGM_PATH, &pgm_a);

    // Load it — the native part must win.
    let loaded =
        CanvasModel::load("h", &hybrid_paged, CanvasOptions::default()).expect("load hybrid");

    let d_a = digests(&model_a);
    let d_b =
        digests(&CanvasModel::load("b2", &idml_b, CanvasOptions::default()).expect("load b2"));
    assert_ne!(
        d_a, d_b,
        "the two fixtures must render differently (test integrity)"
    );
    assert_eq!(
        digests(&loaded),
        d_a,
        "load must reconstruct from the native model part (A), not parse the source parts (B)"
    );
}

/// A document with no native part loads exactly as before — the parse path,
/// which keeps the raw source archive.
#[test]
fn plain_source_still_loads_via_the_parse_path() {
    let idml = paged_gen::write_idml(&paged_gen::samples::geometry::build()).expect("gen fixture");
    let direct = CanvasModel::load("d", &idml, CanvasOptions::default()).expect("load source");
    assert!(
        direct
            .scene()
            .source
            .as_ref()
            .is_some_and(|s| !s.entries.is_empty()),
        "the parse path keeps the raw source archive"
    );
    assert!(
        !digests(&direct).is_empty(),
        "must render at least one page"
    );
}

/// `export_paged` auto-embeds the native model part even when the caller never
/// asked for it, so every saved `.paged` is native-first on reload.
#[test]
fn export_auto_embeds_the_native_model_part() {
    let idml = paged_gen::write_idml(&paged_gen::samples::geometry::build()).expect("gen fixture");
    // No `refresh_model_part` call — export must embed it on its own.
    let model = CanvasModel::load("x", &idml, CanvasOptions::default()).expect("load");
    let paged = model.export_paged(protocol()).expect("export .paged");

    let reloaded = CanvasModel::load("x2", &paged, CanvasOptions::default()).expect("reload");
    assert!(
        reloaded.read_model_part().is_some(),
        "export_paged must auto-embed document.pgm"
    );
    // Native-first reload renders identically to the source-parsed original.
    assert_eq!(
        digests(&reloaded),
        digests(&model),
        "auto-embedded native reload diverges from the original"
    );
}
