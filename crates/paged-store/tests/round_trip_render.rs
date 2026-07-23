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

//! N1 proof (Approach A) — the Paged model **self-owns via native bytes, no
//! IDML**. Parse a generated fixture, serialize the model to native `.paged`
//! bytes, reconstruct a fresh `Document` from those bytes with **no
//! `open_source_archive`**, and assert the two render to identical display-list
//! digests.

use paged_renderer::pipeline::{self, PipelineOptions};

#[test]
fn model_round_trips_through_native_bytes_and_renders_identically() {
    // Import a real fixture — the ONLY IDML parse in this test. The
    // reconstruct path below never touches IDML.
    let idml = paged_gen::write_idml(&paged_gen::samples::geometry::build())
        .expect("generate fixture idml");
    let doc = paged_parse::import_idml_doc(&idml).expect("import fixture");

    // Native round-trip: model -> bytes -> model (no open_source_archive).
    let bytes = paged_store::to_bytes(&doc).expect("serialize to .pgm");
    let doc2 = paged_store::from_bytes(&bytes).expect("reconstruct from .pgm");

    // The reconstructed model carries NO raw IDML source archive by construction
    // — the scene `Document` has no archive field at all (N9) — yet is fully
    // usable.
    assert_eq!(
        doc.spreads.len(),
        doc2.spreads.len(),
        "spread count diverged"
    );
    assert_eq!(
        doc.stories.len(),
        doc2.stories.len(),
        "story count diverged"
    );

    // Both render to identical display-list digests, per page — the proof that
    // the native reconstruction is render-equivalent to the IDML-parsed model.
    let a = pipeline::build_document(&doc, &PipelineOptions::default()).expect("build original");
    let b =
        pipeline::build_document(&doc2, &PipelineOptions::default()).expect("build reconstructed");
    assert_eq!(a.pages.len(), b.pages.len(), "page count diverged");
    assert!(!a.pages.is_empty(), "fixture must render at least one page");
    for (i, (pa, pb)) in a.pages.iter().zip(b.pages.iter()).enumerate() {
        assert_eq!(
            pa.list.digest(),
            pb.list.digest(),
            "page {i}: native round-trip diverges from the IDML-parsed render"
        );
    }
}

/// A part stamped with an incompatible `PGM_FORMAT_VERSION` is rejected (→ the
/// loader falls back to the IDML import), never mis-deserialized (ADR-022 Q2).
#[test]
fn incompatible_format_version_is_rejected() {
    let idml = paged_gen::write_idml(&paged_gen::samples::geometry::build())
        .expect("generate fixture idml");
    let doc = paged_parse::import_idml_doc(&idml).expect("import fixture");
    let bytes = paged_store::to_bytes(&doc).expect("serialize to .pgm");

    // Current version round-trips.
    assert!(
        paged_store::from_bytes(&bytes).is_some(),
        "current PGM_FORMAT_VERSION must load"
    );

    // Bump the envelope's version → incompatible → None.
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).expect("parse envelope");
    v["format_version"] = serde_json::json!(paged_store::PGM_FORMAT_VERSION + 1);
    let future = serde_json::to_vec(&v).expect("reserialize");
    assert!(
        paged_store::from_bytes(&future).is_none(),
        "an incompatible format version must be rejected"
    );

    // Garbage bytes are also rejected (not a panic).
    assert!(paged_store::from_bytes(b"not json").is_none());
}
