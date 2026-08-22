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

// ── C-35 (v62) — moving an item BETWEEN layers ──────────────────────
//
// Until protocol 62 this was inexpressible: the seven `Layer*`
// mutations managed the layer LIST, and nothing wrote which layer an
// item was on. `layers-z.idml` is the fixture that proves it matters —
// a black rectangle on "Background" and a red one on "Foreground",
// overlapping, and the renderer sorts by layer before it paints. Move
// the black one up and the occlusion inverts.

use paged_canvas::channel::Mutation;
use paged_canvas::element_selection::ElementId;
use paged_mutate::{PropertyPath, Value};

/// The two rectangles the fixture ships, with the layer each is on.
fn rect_layers(m: &CanvasModel) -> Vec<(String, Option<String>)> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|p| p.spread.rectangles.iter())
        .map(|r| (r.self_id.clone().unwrap_or_default(), r.item_layer.clone()))
        .collect()
}

#[test]
fn an_item_can_be_moved_to_another_layer_and_undone() {
    let mut m = CanvasModel::load("d", &layered_doc(), CanvasOptions::default()).unwrap();

    let before = rect_layers(&m);
    assert_eq!(before.len(), 2, "layers-z.idml ships two rectangles");
    // The fixture's whole point: the two are on DIFFERENT layers. If a
    // regeneration ever flattened it, this test would otherwise pass
    // while proving nothing.
    let (back_id, back_layer) = before[0].clone();
    let (_, front_layer) = before[1].clone();
    assert_ne!(
        back_layer, front_layer,
        "fixture must place the rectangles on different layers"
    );
    let target = front_layer.clone().expect("front rect carries a layer ref");

    let id = ElementId::Rectangle(back_id.clone());
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::ItemLayer,
        value: Value::Text(target.clone()),
    })
    .expect("ItemLayer accepted");

    let after = rect_layers(&m);
    assert_eq!(
        after[0].1,
        Some(target.clone()),
        "the item did not land on the target layer"
    );

    // The read half must see it too — a Layers panel reads back through
    // `element_properties`, and a write the read half cannot observe is
    // how a panel ends up showing stale truth.
    let seen = m
        .element_properties(&id)
        .expect("props")
        .entries
        .iter()
        .find(|e| e.path == PropertyPath::ItemLayer)
        .and_then(|e| e.value.clone());
    assert_eq!(
        seen,
        Some(Value::Text(target.clone())),
        "read half did not observe the layer write"
    );

    assert!(m.undo().is_some(), "undo produced no outcome");
    assert_eq!(
        rect_layers(&m),
        before,
        "one undo must restore the previous layer exactly"
    );
}

#[test]
fn clearing_the_layer_reference_moves_an_item_to_the_default_layer() {
    let mut m = CanvasModel::load("d", &layered_doc(), CanvasOptions::default()).unwrap();
    let (id_raw, had) = rect_layers(&m)[0].clone();
    assert!(had.is_some(), "fixture rect starts on a named layer");

    // Empty string clears, matching `AppliedObjectStyle`'s convention
    // rather than inventing a null form for one path.
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::Rectangle(id_raw.clone()),
        path: PropertyPath::ItemLayer,
        value: Value::Text(String::new()),
    })
    .expect("empty ItemLayer accepted");

    assert_eq!(
        rect_layers(&m)[0].1,
        None,
        "an empty layer ref must clear to None, not store \"\""
    );
}

#[test]
fn a_group_has_no_layer_of_its_own() {
    // IDML puts `ItemLayer` on leaf items; a `<Group>`'s members each
    // carry their own. Refusing here is the honest answer — silently
    // accepting would let a Layers panel believe it had moved a group
    // whose members never moved.
    let mut m = CanvasModel::load("d", &layered_doc(), CanvasOptions::default()).unwrap();
    let err = m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::Group("does-not-matter".to_string()),
        path: PropertyPath::ItemLayer,
        value: Value::Text("ud81".to_string()),
    });
    assert!(err.is_err(), "a group must not accept an ItemLayer write");
}
