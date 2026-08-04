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

//! v60 — `LayerSummary.parent_id`, the layer-GROUP parentage the wire had
//! been dropping.
//!
//! `paged_model::Layer` has carried `parent_id` all along (the renderer
//! resolves visibility/lock THROUGH ancestors with it), but the wire
//! summary listed six fields and not that one. Every consumer therefore
//! saw a FLAT layer list, which is why a Layers panel could not render a
//! tree — found while implementing ADR 023's shared-panel seam, where it
//! read as "core work" until the model turned out to already have it.
//!
//! The invariant that matters is not "the field exists" but "the summary
//! does not LOSE what the model holds" — a field can be re-dropped by a
//! future refactor of the mapper and nothing else would notice. So the
//! test compares the summaries against the model rather than against a
//! literal.

use paged_canvas::{CanvasModel, CanvasOptions};

fn layered_doc() -> Vec<u8> {
    // Integration tests run with CWD = the CRATE dir, not the workspace
    // root, so the corpus path is anchored to the manifest.
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated/layers-z.idml");
    std::fs::read(&p).unwrap_or_else(|e| panic!("layers-z.idml fixture at {}: {e}", p.display()))
}

#[test]
fn layer_summaries_carry_the_model_parentage() {
    let m = CanvasModel::load("d", &layered_doc(), CanvasOptions::default()).unwrap();
    let summaries = m.layers();
    let model_layers = &m.scene().designmap.layers;

    assert_eq!(
        summaries.len(),
        model_layers.len(),
        "every model layer must reach the wire"
    );

    // The load-bearing assertion: parentage survives the mapping. Written
    // as a comparison rather than `assert!(is_none())` so that adding a
    // nested fixture later strengthens this test without editing it, and
    // so that re-dropping the field fails here rather than silently
    // flattening someone's Layers panel.
    for (summary, layer) in summaries.iter().zip(model_layers.iter()) {
        assert_eq!(
            summary.self_id, layer.self_id,
            "summaries must stay in designmap order"
        );
        assert_eq!(
            summary.parent_id, layer.parent_id,
            "layer {} lost its parent_id crossing the wire",
            layer.self_id
        );
    }
}

#[test]
fn a_flat_document_reports_no_parentage() {
    // The corpus fixture is two peer layers — the overwhelmingly common
    // shape. Pinned separately so the comparison test above cannot pass
    // vacuously if `layers()` ever returned an empty vec.
    let m = CanvasModel::load("d", &layered_doc(), CanvasOptions::default()).unwrap();
    let summaries = m.layers();
    assert!(!summaries.is_empty(), "fixture must contribute layers");
    assert!(
        summaries.iter().all(|s| s.parent_id.is_none()),
        "layers-z.idml declares peer layers; none is nested"
    );
}
