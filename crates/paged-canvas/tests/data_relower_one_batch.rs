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

//! paged.data Wave 8, rows 5 and 6 — what a data LOWER and a data
//! RE-LOWER need from the wire, pinned as the exact batches a bundle sends.
//! Nothing here is new engine surface: the C-15 handles (v57) already
//! address a text frame's minted STORY (`storyId: "$h:f"`, so `insertText`
//! and `insertField` reach a story the same batch created) and an
//! `insertTable`'s minted TABLE (`cell.tableId: "$h:t"`), and `deleteTable`
//! (v66) plus `deleteRange` clear what a previous lower placed. So:
//!
//! - a first lower — frame, text, field, table, cell pours, the frame's
//!   binding label and the document label — is ONE batch, ONE undo step;
//! - a re-lower updates the SAME frame and story in place (no duplicate
//!   frame), also as one batch and one undo step.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use serde_json::json;

const PLUGIN: &str = "media.paged.data";
const LABEL: &str = r#"{"v":1,"data":{"binding":"products"}}"#;

fn model() -> CanvasModel {
    let bytes = paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap();
    CanvasModel::load("relower", &bytes, CanvasOptions::default()).expect("load")
}

fn batch(ops: serde_json::Value) -> Mutation {
    serde_json::from_value(json!({ "op": "batch", "args": { "ops": ops } })).expect("wire spelling")
}

fn cell(table: &str, row: u32, col: u32) -> serde_json::Value {
    json!({ "tableId": table, "row": row, "col": col })
}

fn body_text(m: &CanvasModel, story: &str) -> String {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    s.story
        .paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tables(m: &CanvasModel, story: &str) -> Vec<(String, usize, usize, String)> {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    s.story
        .paragraphs
        .iter()
        .filter_map(|p| p.table.as_ref())
        .map(|t| {
            let first = t
                .cells
                .iter()
                .find(|c| c.name.as_deref() == Some("0:0"))
                .map(|c| {
                    c.paragraphs
                        .iter()
                        .flat_map(|p| p.runs.iter().map(|r| r.text.clone()))
                        .collect::<String>()
                })
                .unwrap_or_default();
            (
                t.self_id.clone().unwrap_or_default(),
                t.rows.len(),
                t.columns.len(),
                first,
            )
        })
        .collect()
}

fn first_lower(page: &str) -> Mutation {
    batch(json!([
        { "op": "insertTextFrame", "args": { "pageId": page, "bounds": [40.0, 40.0, 300.0, 400.0] } },
        { "op": "bindCreated", "args": { "handle": "f" } },
        { "op": "insertText", "args": { "storyId": "$h:f", "offset": 0, "text": "Price: " } },
        { "op": "insertField", "args": { "storyId": "$h:f", "offset": 7,
            "field": { "placeholder": { "plugin": PLUGIN, "key": "price", "value": "9" } } } },
        { "op": "insertTable", "args": { "storyId": "$h:f", "rows": 2, "cols": 2 } },
        { "op": "bindCreated", "args": { "handle": "t" } },
        { "op": "insertText", "args": { "storyId": "$h:f", "offset": 0, "text": "A1", "cell": cell("$h:t", 0, 0) } },
        { "op": "insertText", "args": { "storyId": "$h:f", "offset": 0, "text": "B2", "cell": cell("$h:t", 1, 1) } },
        { "op": "setPluginMetadata", "args": { "elementId": { "kind": "textFrame", "id": "$h:f" },
            "key": format!("x-paged:{PLUGIN}"), "value": LABEL } },
        { "op": "setDocumentMetadata", "args": { "key": format!("x-paged:{PLUGIN}"), "value": LABEL } },
    ]))
}

/// Row 6 — the whole first lower is one batch and one undo step, and the
/// outcome names the frame's story without a collection read.
#[test]
fn a_first_lower_is_one_batch_and_one_undo_step() {
    let mut m = model();
    let page = m.built().pages[0].id.0.clone();
    let log = m.applied_log_len();
    let stories = m.scene().stories.len();
    let out = m.apply_mutation(&first_lower(&page)).expect("first lower");
    assert_eq!(m.applied_log_len(), log + 1, "one undo step");
    let story = out.minted[0]
        .story_id
        .clone()
        .expect("the frame's mint names its story");
    assert_eq!(body_text(&m, &story), "Price: 9\n");
    let t = tables(&m, &story);
    assert_eq!((t.len(), t[0].1, t[0].2, t[0].3.as_str()), (1, 2, 2, "A1"));
    assert_eq!(m.document_placeholders().len(), 1);
    assert_eq!(m.document_meta().plugin_metadata.unwrap().len(), 1);
    m.undo().expect("undo");
    assert_eq!(m.scene().stories.len(), stories);
    assert!(m.document_placeholders().is_empty());
    assert_eq!(m.document_meta().plugin_metadata, Some(vec![]));
}

/// Row 5 — a re-lower updates the SAME frame and story: clear the old
/// table and text, pour the new content, resize by placing a table of the
/// new shape. One batch, one undo step, no second frame.
#[test]
fn a_relower_updates_the_frame_in_place_as_one_undo_step() {
    let mut m = model();
    let page = m.built().pages[0].id.0.clone();
    let out = m.apply_mutation(&first_lower(&page)).expect("first lower");
    let frame = out.minted[0].element.raw_id().to_string();
    let story = out.minted[0].story_id.clone().unwrap();
    let table = tables(&m, &story)[0].0.clone();
    let frames = m.scene().spreads[0].spread.text_frames.len();
    let log = m.applied_log_len();

    // The bundle knows what it placed; the body length after the table's
    // host paragraph is gone is the `insertText` unit (bytes + `\n`s).
    let old_body = "Price: 9";
    let relower = batch(json!([
        { "op": "deleteTable", "args": { "storyId": story, "tableId": table } },
        { "op": "deleteRange", "args": { "storyId": story, "start": 0, "end": old_body.len() } },
        { "op": "insertText", "args": { "storyId": story, "offset": 0, "text": "Preis: " } },
        { "op": "insertField", "args": { "storyId": story, "offset": 7,
            "field": { "placeholder": { "plugin": PLUGIN, "key": "price", "value": "12" } } } },
        { "op": "insertTable", "args": { "storyId": story, "rows": 3, "cols": 1 } },
        { "op": "bindCreated", "args": { "handle": "t" } },
        { "op": "insertText", "args": { "storyId": story, "offset": 0, "text": "Z", "cell": cell("$h:t", 0, 0) } },
        { "op": "setDocumentMetadata", "args": { "key": format!("x-paged:{PLUGIN}"),
            "value": r#"{"v":1,"data":{"binding":"products","rev":2}}"# } },
    ]));
    m.apply_mutation(&relower).expect("re-lower");
    assert_eq!(m.applied_log_len(), log + 1, "one undo step");
    assert_eq!(
        m.scene().spreads[0].spread.text_frames.len(),
        frames,
        "no duplicate frame"
    );
    assert_eq!(body_text(&m, &story), "Preis: 12\n");
    let t = tables(&m, &story);
    assert_eq!((t.len(), t[0].1, t[0].2, t[0].3.as_str()), (1, 3, 1, "Z"));
    assert_eq!(m.document_placeholders().len(), 1);
    assert!(m.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .any(|f| f.self_id.as_deref() == Some(&frame)));

    m.undo().expect("undo the re-lower");
    assert_eq!(body_text(&m, &story), "Price: 9\n");
    let t = tables(&m, &story);
    assert_eq!((t[0].1, t[0].2, t[0].3.as_str()), (2, 2, "A1"));
    assert_eq!(m.document_placeholders()[0].value.as_deref(), Some("9"));
}
