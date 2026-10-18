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

//! How many table rows a frame holds at the edge (ADR 177), over the
//! generated `table-fit.idml`, against InDesign 20.0.1 opening the same
//! file (`table-fit-probe.sh` with `PROBE_IN`, 2026-10-17: each row's
//! frame): a frame holds its rows when its text area — above its bottom
//! inset, inside its stroke's share — holds them and half the strokes
//! above and below them.

use paged_gen::samples::table_fit::{body_story_id, cases, table_id};
use paged_renderer::pipeline::{self, CellAddr};
use paged_renderer::PipelineOptions;

/// Frame B starts here (x, page space); frame A ends at 192.
const FRAME_B_X: f32 = 300.0;

fn open_sans() -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts/OpenSans.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read OpenSans.ttf: {e}"))
}

#[test]
fn each_frame_holds_the_rows_indesign_sets_in_it() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::table_fit::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = open_sans();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let mut report = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let story = body_story_id(i as u32);
        let in_a = (0..8u32)
            .filter(|&row| {
                let addr = CellAddr {
                    table_id: table_id(i as u32),
                    row,
                    col: 0,
                };
                built.pages[i]
                    .story_layout
                    .iter()
                    .find(|l| l.story_id == story && l.cell.as_ref() == Some(&addr))
                    .and_then(|l| l.clusters.first())
                    .is_some_and(|c| c.x_pt < FRAME_B_X)
            })
            .count() as u32;
        if in_a != case.rows_in_a {
            report.push(format!(
                "{}: engine {in_a} rows in frame A, InDesign {}",
                case.name, case.rows_in_a
            ));
        }
    }
    assert!(report.is_empty(), "\n{}", report.join("\n"));
}
