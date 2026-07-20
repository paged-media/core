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

//! Migration slice **S7** — the composition part (`document.pgd`) round-trips
//! through the `.paged` container.
//!
//! The S6 composition model kernel becomes a real on-disk part of the new file
//! format: `CanvasModel::refresh_composition_part` serializes the derived
//! `document.pgd` into `paged/core/composition/…`, `export_paged` writes it
//! into the container, and after a fresh reload `read_composition_part` reads
//! it back byte-faithfully. Additive over the v51 parts door — no protocol
//! bump. The part is a persisted *derived* projection (IDML stays the model).

use paged_canvas::{blank::blank_idml, CanvasModel, CanvasOptions};
use paged_composition::DOCUMENT_PGD_PATH;

/// The protocol the container is stamped with — the current wire version, so
/// the round-trip reflects a same-version reader (no data-loss-guard downgrade).
fn protocol() -> u32 {
    paged_canvas::channel::PROTOCOL_VERSION.0
}

#[test]
fn composition_part_round_trips_through_the_paged_container() {
    // A minimal real document — File▸New synthesises a valid one-page IDML.
    let idml = blank_idml(612.0, 792.0);
    let mut model =
        CanvasModel::load("doc-s7", &idml, CanvasOptions::default()).expect("load blank");

    // Derive the composition, then persist it into the container overlay.
    let derived = model.composition();
    model
        .refresh_composition_part()
        .expect("refresh composition part");

    // The part is now visible under the core-owned namespace.
    assert!(
        model
            .list_paged_parts("paged/core/")
            .iter()
            .any(|p| p == DOCUMENT_PGD_PATH),
        "document.pgd should be listed under paged/core/"
    );

    // Export a real `.paged` container, then reload it from scratch.
    let bytes = model.export_paged(protocol()).expect("export .paged");
    let reloaded = CanvasModel::load("doc-s7-reloaded", &bytes, CanvasOptions::default())
        .expect("reload .paged");

    // The persisted part reads back and deserializes after the round-trip...
    let persisted = reloaded
        .read_composition_part()
        .expect("document.pgd present + deserializes after reload");

    // ...identical to the pre-export derivation, and to a fresh derivation from
    // the reloaded scene — the on-disk part is a faithful projection, not drift.
    assert_eq!(
        persisted, derived,
        "persisted composition != pre-export derived"
    );
    assert_eq!(
        persisted,
        reloaded.composition(),
        "persisted composition != reloaded-scene derivation"
    );
}

#[test]
fn read_composition_part_is_none_when_absent() {
    let idml = blank_idml(612.0, 792.0);
    let model =
        CanvasModel::load("doc-s7-none", &idml, CanvasOptions::default()).expect("load blank");
    // No `refresh_composition_part` was called → the part was never written.
    assert!(model.read_composition_part().is_none());
}
