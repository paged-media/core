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

//! N2 — the native model part (`document.pgm`) round-trips through the `.paged`
//! container and reconstructs a render-equivalent model with no source parse.
//!
//! `refresh_model_part` serializes the whole model into
//! `paged/core/model/document.pgm` over the v51 parts door; after
//! `export_paged` + a fresh reload, `read_model_part` reconstructs the model
//! from that part (no `open_source_archive`) and it renders identically to the
//! reloaded, source-parsed scene. Additive — no load-path change yet.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_renderer::pipeline::{self, PipelineOptions};
use paged_store::DOCUMENT_PGM_PATH;

fn protocol() -> u32 {
    paged_canvas::channel::PROTOCOL_VERSION.0
}

#[test]
fn model_part_round_trips_through_the_paged_container() {
    let idml = paged_gen::write_idml(&paged_gen::samples::geometry::build())
        .expect("generate fixture idml");
    let mut model =
        CanvasModel::load("doc-n2", &idml, CanvasOptions::default()).expect("load fixture");

    // Persist the whole model natively, then confirm it is on the parts index.
    model.refresh_model_part().expect("refresh model part");
    assert!(
        model
            .list_paged_parts("paged/core/")
            .iter()
            .any(|p| p == DOCUMENT_PGM_PATH),
        "document.pgm should be listed under paged/core/"
    );

    // Export a real `.paged`, reload it, and reconstruct the model from the
    // native part (no `open_source_archive` for the model fields).
    let bytes = model.export_paged(protocol()).expect("export .paged");
    let reloaded = CanvasModel::load("doc-n2-reloaded", &bytes, CanvasOptions::default())
        .expect("reload .paged");
    let reconstructed = reloaded
        .read_model_part()
        .expect("document.pgm present + deserializes after reload");

    // Native reconstruct carries no raw source archive...
    assert!(
        reconstructed.source.is_none(),
        "native reconstruct must not carry the raw source archive"
    );

    // ...and renders identically to the reloaded, source-parsed scene.
    let from_pgm = pipeline::build_document(&reconstructed, &PipelineOptions::default())
        .expect("build reconstructed");
    let from_source = pipeline::build_document(reloaded.scene(), &PipelineOptions::default())
        .expect("build reloaded scene");
    assert_eq!(
        from_pgm.pages.len(),
        from_source.pages.len(),
        "page count diverged"
    );
    assert!(!from_pgm.pages.is_empty(), "fixture must render a page");
    for (i, (a, b)) in from_pgm
        .pages
        .iter()
        .zip(from_source.pages.iter())
        .enumerate()
    {
        assert_eq!(
            a.list.digest(),
            b.list.digest(),
            "page {i}: .pgm reconstruction diverges from the source-parsed render"
        );
    }
}

#[test]
fn read_model_part_is_none_when_absent() {
    let idml = paged_gen::write_idml(&paged_gen::samples::geometry::build())
        .expect("generate fixture idml");
    let model =
        CanvasModel::load("doc-n2-none", &idml, CanvasOptions::default()).expect("load fixture");
    // No `refresh_model_part` was called → the part was never written.
    assert!(model.read_model_part().is_none());
}
