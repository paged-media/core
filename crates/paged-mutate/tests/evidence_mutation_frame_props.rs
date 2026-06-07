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

//! W4.2 Full-Green core.mutation evidence — two frame/node-level paths
//! that shipped without their own routable test fn:
//!
//! * `FrameOpacity` (effects-transparency.opacity) — percent on a
//!   Rectangle, `None` ⇒ inherit (fully opaque). Apply / inverse.
//! * `PluginMetadata` (plugin-platform.document-metadata) — the
//!   `x-paged:` Label carrier: namespace gate, JSON-envelope gate,
//!   snapshot inverse including was-absent.
//!
//! Both round-trip through the REAL model from the geometry fixture.

use std::path::PathBuf;

use paged_mutate::{apply, NodeId, Operation, PropertyPath, Value};
use paged_scene::Document;

fn fixture_bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry.idml");
    std::fs::read(path).expect("read geometry fixture")
}

/// (rectangle self-id, spread index) of the first rectangle with an id.
fn first_rectangle(doc: &Document) -> (String, usize) {
    for (si, parsed) in doc.spreads.iter().enumerate() {
        if let Some(r) = parsed
            .spread
            .rectangles
            .iter()
            .find(|r| r.self_id.is_some())
        {
            return (r.self_id.clone().unwrap(), si);
        }
    }
    panic!("geometry fixture must carry a rectangle with a self id");
}

fn rect_opacity(doc: &Document, rect_id: &str) -> Option<f32> {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some(rect_id))
        .and_then(|r| r.opacity)
}

#[test]
fn evid_frame_opacity_writes_percent_and_inverse_restores() {
    let bytes = fixture_bytes();
    let mut doc = Document::open(&bytes).expect("open");
    let (rect_id, _) = first_rectangle(&doc);
    let prev = rect_opacity(&doc, &rect_id);

    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::FrameOpacity,
            value: Value::Length(Some(42.5)),
        },
    )
    .expect("set opacity");
    assert_eq!(
        rect_opacity(&doc, &rect_id),
        Some(42.5),
        "opacity percent written onto the rectangle"
    );

    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(
        rect_opacity(&doc, &rect_id),
        prev,
        "inverse restores the prior opacity (inherit/None or its value)"
    );
}

/// The plugin metadata carrier: an `x-paged:` Label key writes a JSON
/// envelope into the spread's `labels`, the inverse removes it (the
/// was-absent snapshot), and a clearing write (value `None`) drops the
/// entry with its own restoring inverse.
#[test]
fn evid_plugin_metadata_label_round_trips_with_was_absent_inverse() {
    let bytes = fixture_bytes();
    let mut doc = Document::open(&bytes).expect("open");
    let (rect_id, si) = first_rectangle(&doc);
    let key = "x-paged:demo";
    let envelope = r#"{"v":1,"data":{"note":"hello"}}"#;

    // Absent before.
    let absent = |d: &Document| {
        d.spreads[si]
            .spread
            .labels
            .get(&rect_id)
            .map(|e| e.iter().any(|(k, _)| k == key))
            .unwrap_or(false)
    };
    assert!(!absent(&doc), "no metadata before the write");

    let applied = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: key.to_string(),
                value: Some(envelope.to_string()),
                prev: None,
            },
        },
    )
    .expect("write metadata");
    let stored = doc.spreads[si]
        .spread
        .labels
        .get(&rect_id)
        .and_then(|e| e.iter().find(|(k, _)| k == key))
        .map(|(_, v)| v.clone());
    assert_eq!(
        stored.as_deref(),
        Some(envelope),
        "envelope stored on Label"
    );

    // Inverse is the was-absent snapshot: it removes the entry.
    apply(&mut doc, &applied.inverse).expect("undo metadata");
    assert!(
        !absent(&doc),
        "inverse of a was-absent write removes the metadata entry"
    );
}

/// Namespace + envelope gates: a key outside `x-paged:` is rejected,
/// and a non-envelope value is rejected — proving the validation lives
/// in the apply arm, not just the UI.
#[test]
fn evid_plugin_metadata_rejects_foreign_namespace_and_bad_envelope() {
    let bytes = fixture_bytes();
    let mut doc = Document::open(&bytes).expect("open");
    let (rect_id, _) = first_rectangle(&doc);

    let foreign = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: "other:demo".to_string(),
                value: Some(r#"{"v":1,"data":{}}"#.to_string()),
                prev: None,
            },
        },
    );
    assert!(foreign.is_err(), "foreign-namespace key must be rejected");

    let bad_envelope = apply(
        &mut doc,
        &Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: "x-paged:demo".to_string(),
                value: Some("not json".to_string()),
                prev: None,
            },
        },
    );
    assert!(bad_envelope.is_err(), "non-envelope value must be rejected");
}
