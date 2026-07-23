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

//! "Same code, same scene" — the viewer-concept keystone, native
//! form. `ViewerSession::load` runs `paged_sdk::viewer_build`; this
//! test runs the SAME function natively over corpus fixtures and
//! asserts every page's `DisplayList::digest()` matches a direct
//! stock `pipeline::build_document`. If the viewer load path ever
//! diverges from the engine (different options, a "lite" layout
//! shortcut, a skipped pass), this trips. The cross-ARTIFACT lane
//! (browser wasm digest vs native digest) is the recorded V2
//! follow-up.

use std::path::PathBuf;

use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

fn fixture(name: &str) -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join(name);
    std::fs::read(path).ok()
}

#[test]
fn viewer_build_matches_stock_build_document_for_every_page() {
    // Fixtures cover paths/groups, gradients, text, images — the
    // generated set is gitignored and produced by paged-gen, so skip
    // (don't fail) any that are absent locally; CI generates them.
    let names = [
        "geometry-groups.idml",
        "gradients.idml",
        "text.idml",
        "images.idml",
        "transparency.idml",
    ];
    let mut checked = 0;
    for name in names {
        let Some(bytes) = fixture(name) else {
            eprintln!("fixture {name} absent — skipped");
            continue;
        };
        let document = paged_parse::import_idml_doc(&bytes).expect("open fixture");

        let viewer =
            paged_sdk::viewer_build(&document, None, &BytesResolver::new()).expect("viewer_build");
        let stock =
            pipeline::build_document(&document, &PipelineOptions::default()).expect("stock build");

        assert_eq!(
            viewer.pages.len(),
            stock.pages.len(),
            "{name}: page count diverges"
        );
        for (i, (v, s)) in viewer.pages.iter().zip(stock.pages.iter()).enumerate() {
            assert_eq!(
                v.list.digest(),
                s.list.digest(),
                "{name} page {i}: viewer load path diverges from stock build_document"
            );
        }
        checked += 1;
    }
    assert!(
        checked > 0,
        "no fixtures present — run paged-gen or check corpus/generated"
    );
}

#[test]
fn digest_is_order_and_content_sensitive() {
    let Some(bytes) = fixture("geometry-groups.idml") else {
        eprintln!("fixture absent — skipped");
        return;
    };
    let document = paged_parse::import_idml_doc(&bytes).expect("open fixture");
    let built =
        paged_sdk::viewer_build(&document, None, &BytesResolver::new()).expect("viewer_build");

    // Re-building yields identical digests (determinism)…
    let again = paged_sdk::viewer_build(&document, None, &BytesResolver::new())
        .expect("viewer_build again");
    for (a, b) in built.pages.iter().zip(again.pages.iter()) {
        assert_eq!(
            a.list.digest(),
            b.list.digest(),
            "build must be deterministic"
        );
    }
    // …and two different pages hash differently (sanity that the
    // digest actually reads the content).
    if built.pages.len() >= 2 {
        assert_ne!(
            built.pages[0].list.digest(),
            built.pages[1].list.digest(),
            "distinct pages should not collide"
        );
    }
}
