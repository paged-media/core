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

//! A page minted in a batch is named by its folio, as InDesign names it.
//!
//! InDesign writes each page's folio as its `Name` ("1", "iv", "A-2") and
//! gives a page it adds or duplicates the folio where it lands. Here a
//! duplicated page kept its source's `Name`, so every page a Data Merge
//! duplicated from the template was page "1" (and printed "1" in a
//! page-number marker), and an inserted page had no name at all: the
//! outline called it "Page" and a host had only the page's id ("u5") to
//! show. A duplicate drops the copied folio now, and the reads report each
//! page's folio: its baked `Name`, else the number the section rules give
//! it.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use serde_json::json;

/// InDesign's one-page Data Merge template; its page `ud4` is `Name="1"`.
fn template() -> CanvasModel {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/data-merge-empty-field-lines.idml");
    CanvasModel::load(
        "merge",
        &std::fs::read(p).expect("fixture"),
        CanvasOptions::default(),
    )
    .expect("load")
}

fn merge_like_batch(m: &mut CanvasModel) {
    let page = m.built().pages[0].id.0.clone();
    let batch: Mutation = serde_json::from_value(json!({ "op": "batch", "args": { "ops": [
        { "op": "duplicatePage", "args": { "page": page } },
        { "op": "bindCreated", "args": { "handle": "a" } },
        { "op": "duplicatePage", "args": { "page": "$h:a" } },
        { "op": "insertPage", "args": { "afterPageId": null, "masterId": null } },
    ] } }))
    .expect("wire spelling");
    m.apply_mutation(&batch).expect("batch");
}

#[test]
fn pages_minted_in_a_batch_read_as_page_numbers() {
    let mut m = template();
    merge_like_batch(&mut m);
    assert_eq!(m.built().pages.len(), 4);

    // The outline names each page by its folio.
    let outline: Vec<String> = m
        .scene_tree()
        .iter()
        .flat_map(|spread| spread.children.iter().filter(|c| c.kind == "Page"))
        .map(|p| p.label.clone())
        .collect();
    assert_eq!(outline, ["1", "2", "3", "4"], "the outline's page labels");

    // So does the pages collection, whatever the ids are.
    let names: Vec<String> = m.pages().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["1", "2", "3", "4"], "the pages' names");

    // The copies carry no stale folio of their own.
    let baked: Vec<Option<String>> = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.pages.iter().map(|p| p.name.clone()))
        .collect();
    assert_eq!(baked, [Some("1".to_string()), None, None, None]);
}
