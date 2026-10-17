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

//! A table nested in a table cell (ADR 176), over the generated
//! `table-nested.idml`, against where InDesign 20.0.1 set its words in
//! its PDF of the same file (2026-10-17, `pdftotext -bbox`; the fixture
//! is also in the fidelity gate with that PDF as reference).
//!
//! InDesign sets a nested table as it sets any table: its rows grown to
//! their content, its strokes, fills and spans, and its columns at their
//! declared widths — past the cell's edge when wider. The host row under
//! it starts where the nested table and the cell's insets end.

use paged_gen::samples::table_nested::{body_story_id, cases, host_table_id};
use paged_renderer::pipeline::{self, CellAddr};
use paged_renderer::PipelineOptions;

const TOLERANCE: f32 = 0.25;

/// Per case: how far below the nested table's first line InDesign set the
/// host row's line (`host` minus `A1`, top of the word box), and how far
/// right of column A its last column starts (`D1`/`C2`/`B2` minus `A1`).
const INDESIGN: &[(f32, f32)] = &[
    (196.2287 - 129.2502, 241.0 - 81.0),
    (196.2287 - 129.2502, 241.0 - 81.0),
    (196.2287 - 129.2502, 241.0 - 81.0),
    (217.0548 - 129.2502, 241.0 - 81.0),
    (175.4025 - 129.2502, 131.0 - 81.0),
    (175.4025 - 129.2502, 381.0 - 81.0),
];

fn open_sans() -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts/OpenSans.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read OpenSans.ttf: {e}"))
}

fn build() -> pipeline::BuiltDocument {
    let bytes = paged_gen::write_idml(&paged_gen::samples::table_nested::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = open_sans();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&doc, &opts).expect("build")
}

#[test]
fn a_nested_table_takes_the_room_indesign_gives_it() {
    let built = build();
    let mut report = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let page = &built.pages[i];
        let story = body_story_id(i as u32);
        let host_row = CellAddr {
            table_id: host_table_id(i as u32),
            row: 1,
            col: 0,
        };
        let host_y = page
            .story_layout
            .iter()
            .find(|l| l.story_id == story && l.cell.as_ref() == Some(&host_row))
            .map(|l| l.baseline_y_pt)
            .expect("the host row's line");
        // The nested table's cells: lines of the host story with no cell
        // address (nested-cell editing is deferred).
        let nested: Vec<_> = page
            .story_layout
            .iter()
            .filter(|l| l.story_id == story && l.cell.is_none() && !l.clusters.is_empty())
            .collect();
        let first_y = nested
            .iter()
            .map(|l| l.baseline_y_pt)
            .fold(f32::INFINITY, f32::min);
        let starts: Vec<f32> = nested.iter().map(|l| l.clusters[0].x_pt).collect();
        let span_x = starts.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - starts.iter().copied().fold(f32::INFINITY, f32::min);
        let (want_dy, want_dx) = INDESIGN[i];
        let (dy, dx) = (host_y - first_y, span_x);
        if (dy - want_dy).abs() > TOLERANCE || (dx - want_dx).abs() > TOLERANCE {
            report.push(format!(
                "{}: host row {dy:.3} below, last column {dx:.3} right; InDesign {want_dy:.3}, {want_dx:.3}",
                case.name
            ));
        }
    }
    assert!(report.is_empty(), "\n{}", report.join("\n"));
}
