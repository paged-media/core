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

//! The batched table pour: placing a spreadsheet range as a native table
//! is ONE `mutate` and ONE undo step.
//!
//! paged.sheet poured a range one awaited `insertText` per cell, because
//! its notes said `Mutation::Batch` carries frame ops only. That stopped
//! being true with the mixed-lane batch (RFI C-14, one undo step) and its
//! single deferred rebuild; these tests pin the TABLE shape of it. Pinning
//! it found that undoing the first pour into a fresh cell (which carries
//! no paragraph until the pour seeds one) left the seeded paragraph
//! behind, so the undo never returned to the state before the pour.
//!
//! The JSON below is the wire shape the plugin's `tableContentBatch`
//! emits, in structure.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, ElementId};

fn idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 600 500" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Above the table</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn load() -> CanvasModel {
    CanvasModel::load("doc", &idml(), CanvasOptions::default()).expect("load")
}

fn mutation(json: serde_json::Value) -> Mutation {
    serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("decode {json}: {e}"))
}

/// One table's cells, `(col, row)` → text, sorted.
type Cells = Vec<((u32, u32), String)>;

/// Every table in `story`, as `(table_id, cells)`.
fn tables(model: &CanvasModel, story: &str) -> Vec<(String, Cells)> {
    let story = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .unwrap_or_else(|| panic!("story {story}"));
    story
        .story
        .paragraphs
        .iter()
        .filter_map(|p| p.table.as_ref())
        .map(|t| {
            let mut cells: Cells = t
                .cells
                .iter()
                .filter_map(|c| {
                    let text: String = c
                        .paragraphs
                        .iter()
                        .flat_map(|p| p.runs.iter().map(|r| r.text.as_str()))
                        .collect();
                    c.coords().map(|xy| (xy, text))
                })
                .collect();
            cells.sort();
            (t.self_id.clone().unwrap_or_default(), cells)
        })
        .collect()
}

fn cell_text(model: &CanvasModel, story: &str, table: &str, col: u32, row: u32) -> String {
    tables(model, story)
        .into_iter()
        .find(|(id, _)| id == table)
        .and_then(|(_, cells)| {
            cells
                .into_iter()
                .find(|(xy, _)| *xy == (col, row))
                .map(|(_, t)| t)
        })
        .unwrap_or_else(|| panic!("cell ({col},{row}) of {table}"))
}

fn insert_table(model: &mut CanvasModel) -> String {
    let out = model
        .apply_mutation(&mutation(serde_json::json!({
            "op": "insertTable",
            "args": { "storyId": "story1", "rows": 2, "cols": 2,
                      "columnWidths": [100.0, 100.0], "rowHeights": [20.0, 20.0] }
        })))
        .expect("insertTable");
    match out.created_id {
        Some(ElementId::Table { table_id, .. }) => table_id,
        other => panic!("insertTable reports a Table createdId, got {other:?}"),
    }
}

/// `tableContentBatch`'s shape: the pour (text lane) and the decor (span +
/// a cell-scoped edge stroke, frame lane) in ONE batch.
fn content_batch(table_id: &str) -> Mutation {
    let pour = |row: u32, col: u32, text: &str| {
        serde_json::json!({ "op": "insertText", "args": {
            "storyId": "story1", "offset": 0, "text": text,
            "cell": { "tableId": table_id, "row": row, "col": col } } })
    };
    mutation(serde_json::json!({ "op": "batch", "args": { "ops": [
        pour(0, 0, "Region"),
        pour(0, 1, "Q1"),
        pour(1, 0, "North"),
        pour(1, 1, "1,250"),
        { "op": "setCellSpan", "args": { "storyId": "story1", "tableId": table_id,
            "row": 0, "col": 0, "rowSpan": 1, "columnSpan": 1 } },
        { "op": "setElementProperty", "args": {
            "elementId": { "kind": "tableCell",
                "id": { "story_id": "story1", "table_id": table_id, "row": 0, "col": 0 } },
            "path": "cellBottomEdgeStrokeWeight",
            "value": { "type": "length", "value": 0.5 } } }
    ] } }))
}

#[test]
fn a_range_pour_with_its_decor_is_one_mutate_one_build_one_undo() {
    let mut model = load();
    let table = insert_table(&mut model);
    let hash_before = model.current_state_hash();
    let log_before = model.applied_log_len();
    let builds_before = model.last_rebuild_stats().rebuilds;

    model
        .apply_mutation(&content_batch(&table))
        .expect("the pour + decor batch applies");

    assert_eq!(
        model.last_rebuild_stats().rebuilds - builds_before,
        1,
        "the whole range builds the document once, not once per cell",
    );
    assert_eq!(
        model.applied_log_len() - log_before,
        1,
        "the batch is ONE undo record",
    );
    assert_eq!(cell_text(&model, "story1", &table, 0, 0), "Region");
    assert_eq!(cell_text(&model, "story1", &table, 1, 0), "Q1");
    assert_eq!(cell_text(&model, "story1", &table, 0, 1), "North");
    assert_eq!(cell_text(&model, "story1", &table, 1, 1), "1,250");
    let hash_after = model.current_state_hash();

    model.undo().expect("undo");
    assert_eq!(
        model.current_state_hash(),
        hash_before,
        "one undo takes back every cell AND the decor",
    );
    assert_eq!(cell_text(&model, "story1", &table, 1, 1), "");

    model.redo().expect("redo");
    assert_eq!(
        model.current_state_hash(),
        hash_after,
        "one redo restores it"
    );
}

/// A failing child (a cell outside the table) rolls the whole pour back.
#[test]
fn a_pour_with_a_bad_cell_applies_nothing() {
    let mut model = load();
    let table = insert_table(&mut model);
    let hash_before = model.current_state_hash();
    let err = model
        .apply_mutation(&mutation(
            serde_json::json!({ "op": "batch", "args": { "ops": [
            { "op": "insertText", "args": { "storyId": "story1", "offset": 0, "text": "ok",
                "cell": { "tableId": table, "row": 0, "col": 0 } } },
            { "op": "insertText", "args": { "storyId": "story1", "offset": 0, "text": "nope",
                "cell": { "tableId": table, "row": 9, "col": 9 } } }
        ] } }),
        ))
        .expect_err("a cell outside the table fails the batch");
    assert!(
        format!("{err:?}").contains("child 1"),
        "names the child: {err:?}"
    );
    assert_eq!(model.current_state_hash(), hash_before, "all or nothing");
}

/// A fresh table cell holds no paragraph; the first pour seeds one. Its
/// undo must take the seed back too, or the document after undo carries
/// an empty paragraph it never had (and a save writes it out). Redo
/// seeds it again.
#[test]
fn undoing_the_first_pour_into_a_fresh_cell_restores_the_exact_state() {
    let mut model = load();
    let table = insert_table(&mut model);
    let hash_before = model.current_state_hash();
    let pour = |text: &str| {
        mutation(serde_json::json!({ "op": "insertText", "args": {
            "storyId": "story1", "offset": 0, "text": text,
            "cell": { "tableId": table, "row": 0, "col": 0 } } }))
    };
    for text in ["x", "two\\nlines", ""] {
        let text = text.replace("\\n", "\n");
        model.apply_mutation(&pour(&text)).expect("pour");
        let hash_after = model.current_state_hash();
        model.undo().expect("undo");
        assert_eq!(model.current_state_hash(), hash_before, "undo of {text:?}");
        model.redo().expect("redo");
        assert_eq!(model.current_state_hash(), hash_after, "redo of {text:?}");
        model.undo().expect("undo again");
    }
}
