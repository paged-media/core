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

//! v68 — document-scoped plugin metadata (`setDocumentMetadata`): a label
//! on the DOCUMENT for state that belongs to no frame (paged.data's
//! session, the live version of a plugin's container parts). It must be
//! undoable — that is the whole point: a label that names the live parts
//! version is only true after an undo if undo restores it — compose in a
//! batch as one step, read back through `DocumentMeta`, and survive a
//! `.paged` save and reload.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};

const KEY: &str = "x-paged:media.paged.data";

fn model() -> CanvasModel {
    let bytes = paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap();
    CanvasModel::load("doc-meta", &bytes, CanvasOptions::default()).expect("load")
}

fn set(key: &str, value: Option<&str>) -> Mutation {
    Mutation::SetDocumentMetadata {
        key: key.into(),
        value: value.map(str::to_string),
        caller: None,
    }
}

fn label(m: &CanvasModel, key: &str) -> Option<String> {
    m.document_meta()
        .plugin_metadata
        .expect("a v68 engine always reports document metadata")
        .into_iter()
        .find(|e| e.key == key)
        .map(|e| e.value)
}

const V1: &str = r#"{"v":1,"data":{"parts":"a1"}}"#;
const V2: &str = r#"{"v":1,"data":{"parts":"b2"}}"#;

#[test]
fn a_document_label_is_written_read_back_and_undone() {
    let mut m = model();
    assert_eq!(m.document_meta().plugin_metadata, Some(vec![]));
    m.apply_mutation(&set(KEY, Some(V1))).expect("set");
    assert_eq!(label(&m, KEY).as_deref(), Some(V1));
    m.apply_mutation(&set(KEY, Some(V2))).expect("replace");
    assert_eq!(label(&m, KEY).as_deref(), Some(V2));
    m.undo().expect("undo replace");
    assert_eq!(
        label(&m, KEY).as_deref(),
        Some(V1),
        "undo restores the prior version"
    );
    m.redo().expect("redo replace");
    assert_eq!(label(&m, KEY).as_deref(), Some(V2));
    m.apply_mutation(&set(KEY, None)).expect("delete");
    assert_eq!(label(&m, KEY), None);
    m.undo().expect("undo delete");
    assert_eq!(label(&m, KEY).as_deref(), Some(V2));
    m.undo().expect("undo replace");
    m.undo().expect("undo first write");
    assert_eq!(label(&m, KEY), None, "was absent, is absent again");
}

#[test]
fn the_gates_match_the_page_item_carrier() {
    let mut m = model();
    // Outside the reserved namespace.
    assert!(m.apply_mutation(&set("mine", Some(V1))).is_err());
    // Not the JSON envelope.
    assert!(m.apply_mutation(&set(KEY, Some("plain text"))).is_err());
    // Over the 64 KiB cap.
    let big = format!(r#"{{"v":1,"data":{{"x":"{}"}}}}"#, "a".repeat(64 * 1024));
    assert!(m.apply_mutation(&set(KEY, Some(&big))).is_err());
    // The B-16 caller gate: a plugin may only write its own key.
    let foreign = Mutation::SetDocumentMetadata {
        key: "x-paged:media.paged.web".into(),
        value: Some(V1.into()),
        caller: Some("media.paged.data".into()),
    };
    assert!(m.apply_mutation(&foreign).is_err());
    let own = Mutation::SetDocumentMetadata {
        key: KEY.into(),
        value: Some(V1.into()),
        caller: Some("media.paged.data".into()),
    };
    m.apply_mutation(&own).expect("own namespace");
    assert_eq!(m.document_meta().plugin_metadata.unwrap().len(), 1);
}

#[test]
fn a_document_label_rides_a_batch_as_one_undo_step() {
    let mut m = model();
    let page = m.built().pages[0].id.0.clone();
    let batch: Mutation = serde_json::from_value(serde_json::json!({
        "op": "batch",
        "args": { "ops": [
            { "op": "insertTextFrame", "args": { "pageId": page, "bounds": [40.0, 40.0, 120.0, 300.0] } },
            { "op": "setDocumentMetadata", "args": { "key": KEY, "value": V1 } },
        ] },
    }))
    .expect("wire spelling");
    let frames_before = m.scene().spreads[0].spread.text_frames.len();
    m.apply_mutation(&batch).expect("batch");
    assert_eq!(label(&m, KEY).as_deref(), Some(V1));
    assert_eq!(
        m.scene().spreads[0].spread.text_frames.len(),
        frames_before + 1
    );
    m.undo().expect("one undo");
    assert_eq!(label(&m, KEY), None);
    assert_eq!(m.scene().spreads[0].spread.text_frames.len(), frames_before);
}

#[test]
fn a_document_label_survives_a_paged_save_and_reload() {
    let mut m = model();
    m.apply_mutation(&set(KEY, Some(V1))).expect("set");
    let bytes = m
        .export_paged(paged_canvas::channel::PROTOCOL_VERSION.0)
        .expect("export .paged");
    let reloaded =
        CanvasModel::load("doc-meta-2", &bytes, CanvasOptions::default()).expect("reload");
    assert_eq!(label(&reloaded, KEY).as_deref(), Some(V1));
}
