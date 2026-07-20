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

    // A container that is B's source parts + A's native model part.
    let mut hybrid = CanvasModel::load("b", &idml_b, CanvasOptions::default()).expect("load b");
    hybrid
        .set_paged_part(DOCUMENT_PGM_PATH.to_string(), pgm_a)
        .expect("inject a's model part");
    let hybrid_paged = hybrid.export_paged(protocol()).expect("export hybrid");

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
        !direct.scene().container.entries.is_empty(),
        "the parse path keeps the raw source archive"
    );
    assert!(
        !digests(&direct).is_empty(),
        "must render at least one page"
    );
}
