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

//! Inspector P1 — end-to-end test of the inspector wire surface.
//! Loads a real fixture, requests element properties for a known
//! frame, requests the scene tree, and routes a `SetElementProperty`
//! mutation through the apply layer asserting the round-trip.

use std::path::PathBuf;

use paged_canvas::{
    channel::Mutation,
    element_selection::ElementId,
    CanvasModel, CanvasOptions,
};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml")
}

fn load_model() -> CanvasModel {
    let bytes = std::fs::read(fixture_path()).expect("read fixture");
    let opts = CanvasOptions::default();
    CanvasModel::load("doc-inspector", &bytes, opts).expect("load + build")
}

#[test]
fn scene_tree_lists_every_spread_with_frame_leaves() {
    let model = load_model();
    let roots = model.scene_tree();
    assert!(!roots.is_empty(), "expected at least one spread");
    for spread in &roots {
        assert_eq!(spread.kind, "Spread");
        assert!(!spread.children.is_empty(), "spread should have a Page");
        let page = &spread.children[0];
        assert_eq!(page.kind, "Page");
        // Every page in geometry-groups has at least one frame
        // (the label text frame).
        assert!(
            !page.children.is_empty(),
            "page should have at least one frame child"
        );
    }
}

#[test]
fn element_properties_for_textframe_surfaces_authored_values() {
    let model = load_model();
    // Page-0 label text frame id from inspecting the fixture:
    // `<TextFrame Self="ua365e1" …>`.
    let id = ElementId::TextFrame("ua365e1".to_string());
    let props = model
        .element_properties(&id)
        .expect("element_properties for known text frame");
    assert_eq!(props.kind, "TextFrame");
    // The entry list should include every frame-level path the
    // Inspector renders editors for.
    let paths: Vec<_> = props.entries.iter().map(|e| e.path).collect();
    use paged_mutate::PropertyPath::*;
    for expected in &[
        FrameBounds,
        FrameTransform,
        FrameFillColor,
        FrameStrokeColor,
        FrameStrokeWeight,
        FrameOpacity,
    ] {
        assert!(
            paths.contains(expected),
            "expected {expected:?} in entries; got {paths:?}",
        );
    }
}

#[test]
fn set_element_property_mutation_routes_through_apply_layer() {
    let mut model = load_model();
    let id = ElementId::TextFrame("ua365e1".to_string());
    // Authored opacity is `None` (unset); set it to 50 via the
    // generic SetElementProperty mutation the Inspector uses.
    let mutation = Mutation::SetElementProperty {
        element_id: id.clone(),
        path: paged_mutate::PropertyPath::FrameOpacity,
        value: paged_mutate::Value::Length(Some(50.0)),
    };
    let outcome = model.apply_mutation(&mutation).expect("apply ok");
    assert!(
        outcome.applied_seq > 0,
        "applied_seq should advance after a successful mutation; got {}",
        outcome.applied_seq,
    );
    // Re-fetch properties and confirm the new value is visible.
    let props = model.element_properties(&id).expect("post-apply props");
    let opacity_entry = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::FrameOpacity)
        .expect("opacity entry");
    assert_eq!(
        opacity_entry.value,
        Some(paged_mutate::Value::Length(Some(50.0))),
        "opacity should reflect the mutation",
    );
}
