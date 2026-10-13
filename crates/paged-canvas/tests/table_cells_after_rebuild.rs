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

//! A table's cell rects survive a rebuild that replays its story's cached
//! emission.
//!
//! The cell rects are the hit-test map for table cells (`BuiltPage::
//! cell_rects`): the only place cell geometry exists after the display
//! list flattens a table into fills, strokes and glyphs. A rebuild that
//! does not touch a story (a plugin's scene layer, a registered font, a
//! paint write) splices that story's cached emission instead of laying it
//! out again. The cached delta carried the commands, the text lines and
//! the footnotes, but not the cell rects, so after the first such rebuild
//! the table had no cells: a double-click entered vector editing, a hit
//! never named a cell, and a cell caret reached no panel. An edit that
//! cleared every cache brought them back until the next replay.

use std::path::PathBuf;

use paged_canvas::element_selection::ElementId;
use paged_canvas::{CanvasModel, CanvasOptions};
use paged_compose::{SceneItem, SceneLayer, ScenePaint, ScenePathSeg};

fn inter() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load(sample: &str) -> CanvasModel {
    let s = paged_gen::samples::build(sample).unwrap_or_else(|| panic!("no sample {sample:?}"));
    let bytes = paged_gen::write_idml(&s).expect("write idml");
    let opts = CanvasOptions {
        fonts: vec![inter()],
        ..CanvasOptions::default()
    };
    CanvasModel::load("cells", &bytes, opts).expect("load")
}

/// Every page's cell rects as comparable tuples, in emit order.
fn cells(m: &CanvasModel) -> Vec<(String, String, String, u32, u32, [u32; 4])> {
    m.built()
        .pages
        .iter()
        .flat_map(|p| {
            p.cell_rects.iter().map(move |c| {
                (
                    p.id.as_str().to_string(),
                    c.story_id.clone(),
                    c.table_id.clone(),
                    c.row,
                    c.col,
                    c.rect.map(f32::to_bits),
                )
            })
        })
        .collect()
}

fn a_layer() -> SceneLayer {
    SceneLayer {
        items: vec![SceneItem::FillPath {
            path: vec![
                ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
                ScenePathSeg::LineTo { x: 4.0, y: 0.0 },
                ScenePathSeg::LineTo { x: 4.0, y: 4.0 },
                ScenePathSeg::Close,
            ],
            paint: ScenePaint {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        }],
    }
}

#[test]
fn cell_rects_survive_rebuilds_that_replay_the_table_story() {
    for sample in ["tables", "tables-overset", "tables-rows"] {
        let mut m = load(sample);
        let at_load = cells(&m);
        assert!(!at_load.is_empty(), "{sample}: the table has cells at load");

        // Each scene-layer write rebuilds the document; none of them
        // touches the table's story, so its emission is replayed.
        for i in 0..3 {
            m.set_scene_layer(format!("test.layer.{i}"), a_layer())
                .expect("scene layer + rebuild");
            let now = cells(&m);
            assert_eq!(
                now.len(),
                at_load.len(),
                "{sample}: rebuild {i} replayed the table story without its cell rects"
            );
            assert!(now == at_load, "{sample}: rebuild {i} moved the cell rects");
        }

        // The user-facing read: a cell answers its geometry after the
        // replays, exactly as at load.
        let (_, story_id, table_id, row, col, _) = at_load[0].clone();
        let id = ElementId::TableCell {
            story_id,
            table_id,
            row,
            col,
        };
        assert_eq!(
            m.element_geometry(std::slice::from_ref(&id)).len(),
            1,
            "{sample}: the first cell answers element geometry after the rebuilds"
        );
    }
}

/// A story laid out more than once in one build (a footnote or keeps
/// re-emit pass rolls the page back to where the story started) records
/// each cell once: the rollback truncates the cell rects with the lines.
/// A story with footnotes lays out once more after reserving their
/// space, and a table placed in it must not leave the first pass's cells
/// behind.
#[test]
fn a_table_in_a_footnoted_story_records_each_cell_once() {
    let mut m = load("footnotes");
    let story = m
        .scene()
        .stories
        .iter()
        .find(|s| s.story.paragraphs.iter().any(|p| !p.footnotes.is_empty()))
        .map(|s| s.self_id.clone())
        .expect("the sample has a footnoted story");
    let table: paged_canvas::channel::Mutation = serde_json::from_value(serde_json::json!({
        "op": "insertTable",
        "args": { "storyId": story, "rows": 2, "cols": 2, "rowHeights": [20.0, 20.0] }
    }))
    .expect("wire spelling");
    m.apply_mutation(&table).expect("insert table");
    let all = cells(&m);
    assert!(!all.is_empty(), "the table lays out");
    let mut seen = std::collections::BTreeSet::new();
    let twice = all.iter().filter(|c| !seen.insert(*c)).count();
    assert_eq!(
        twice,
        0,
        "{twice} of {} cell rects recorded twice",
        all.len()
    );
}
