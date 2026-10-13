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

//! A batch that deletes page items and then inserts new ones of the same
//! kind on the same spread applies as one step.
//!
//! The batch translates every child against the scene as it stood before
//! the batch, and an insert appends to its kind's list there: its slot was
//! that list's length. A `deleteFrame` earlier in the same batch shortens
//! the list before the insert applies, so the slot pointed past the end
//! and the whole batch was refused ("position 2 out of range for parent
//! Spread … (len 0)"). paged.data's barcode re-lower is exactly this
//! batch (remove the old bars, draw the new ones); it had to put the new
//! modules first to get through.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use serde_json::{json, Value};

fn model() -> CanvasModel {
    let bytes = paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap();
    CanvasModel::load("relower", &bytes, CanvasOptions::default()).expect("load")
}

fn mutation(v: Value) -> Mutation {
    serde_json::from_value(v).expect("wire spelling")
}

fn batch(ops: Vec<Value>) -> Mutation {
    mutation(json!({ "op": "batch", "args": { "ops": ops } }))
}

fn square(page: &str, x: f32) -> Value {
    let corner =
        |px: f32, py: f32| json!({ "anchor": [px, py], "left": [px, py], "right": [px, py] });
    json!({ "op": "insertPath", "args": { "pageId": page, "open": false, "anchors": [
        corner(x, 40.0), corner(x + 10.0, 40.0), corner(x + 10.0, 50.0), corner(x, 50.0)
    ] } })
}

fn delete(id: &str) -> Value {
    json!({ "op": "deleteFrame", "args": { "frameId": id } })
}

/// Every page item's id of one kind on the first spread, in list order.
fn ids(m: &CanvasModel, kind: &str) -> Vec<String> {
    let s = &m.scene().spreads[0].spread;
    let ids: Vec<Option<String>> = match kind {
        "polygon" => s.polygons.iter().map(|f| f.self_id.clone()).collect(),
        "rectangle" => s.rectangles.iter().map(|f| f.self_id.clone()).collect(),
        "graphicLine" => s.graphic_lines.iter().map(|f| f.self_id.clone()).collect(),
        "textFrame" => s.text_frames.iter().map(|f| f.self_id.clone()).collect(),
        _ => unreachable!(),
    };
    ids.into_iter().flatten().collect()
}

#[test]
fn insert_path_after_delete_frame_in_one_batch() {
    let mut m = model();
    let page = m.built().pages[0].id.0.clone();
    // The bars a first lower drew.
    m.apply_mutation(&batch(vec![square(&page, 40.0), square(&page, 60.0)]))
        .expect("first lower");
    let old = ids(&m, "polygon");
    assert_eq!(old.len(), 2);
    let log = m.applied_log_len();

    // The re-lower: remove the old bars, draw the new ones, one batch.
    let relower = batch(vec![
        delete(&old[0]),
        delete(&old[1]),
        square(&page, 100.0),
        square(&page, 120.0),
        square(&page, 140.0),
    ]);
    m.apply_mutation(&relower)
        .expect("a delete-then-insert batch applies");
    assert_eq!(m.applied_log_len(), log + 1, "one undo step");
    let now = ids(&m, "polygon");
    assert_eq!(now.len(), 3, "three new bars, the old two gone: {now:?}");
    assert!(now.iter().all(|id| !old.contains(id)));

    m.undo().expect("undo");
    assert_eq!(ids(&m, "polygon"), old, "undo restores the old bars");
    m.redo().expect("redo");
    assert_eq!(ids(&m, "polygon").len(), 3);
}

/// The same for every creating op that appends to a spread's kind list.
#[test]
fn every_insert_kind_follows_a_delete_of_its_kind() {
    type Insert = fn(&str) -> Value;
    let cases: [(&str, Insert); 3] = [
        (
            "rectangle",
            |page| json!({ "op": "insertFrame", "args": { "pageId": page, "bounds": [40.0, 40.0, 80.0, 80.0] } }),
        ),
        (
            "graphicLine",
            |page| json!({ "op": "insertLine", "args": { "pageId": page, "start": [40.0, 40.0], "end": [90.0, 40.0] } }),
        ),
        (
            "textFrame",
            |page| json!({ "op": "insertTextFrame", "args": { "pageId": page, "bounds": [40.0, 40.0, 80.0, 200.0] } }),
        ),
    ];
    for (kind, insert) in cases {
        let mut m = model();
        let page = m.built().pages[0].id.0.clone();
        m.apply_mutation(&batch(vec![insert(&page), insert(&page)]))
            .unwrap_or_else(|e| panic!("{kind}: seed: {e:?}"));
        let all = ids(&m, kind);
        let before = all.len();
        let mut ops: Vec<Value> = all.iter().map(|id| delete(id)).collect();
        ops.push(insert(&page));
        m.apply_mutation(&batch(ops))
            .unwrap_or_else(|e| panic!("{kind}: delete-then-insert refused: {e:?}"));
        assert_eq!(
            ids(&m, kind).len(),
            1,
            "{kind}: {before} deleted, one inserted"
        );
    }
}
