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

//! A table row's Start Row option (ADR 175), over the generated
//! `table-start-row.idml`, against where InDesign 20.0.1 set every row of
//! the same file (2026-10-17: `table-start-row-probe.sh` with `PROBE_IN`,
//! each row's first insertion point's frame, page and column; the fixture
//! is also in the fidelity gate with InDesign's PDF as reference).
//!
//! InDesign sets the rule as a paragraph's `StartParagraph`: the row and
//! every row after it move to the next column, frame or page, or to the
//! next page of the asked parity, past an empty frame. The rule on the
//! table's first row at the top of the story, and `NextColumn` on a row
//! that already opens column two, move nothing. A header repeats over the
//! rows the rule moves.

use paged_gen::samples::table_start_row::{body_story_id, cases, table_id, PAGES_PER_CASE};
use paged_renderer::pipeline::{self, CellAddr};
use paged_renderer::PipelineOptions;

/// Where InDesign set each row of each case (header first where there is
/// one): `A1`/`A2` frame A's columns and `B` the frame below it on the
/// case's first page, `C`, `D`, `E` the frames on its pages 2-4.
const INDESIGN: &[&[&str]] = &[
    &["A1", "A1", "A1", "A1", "A1", "A2", "A2", "A2"],
    &["A1", "A1", "A1", "A2", "A2", "A2", "A2", "A2"],
    &["A1", "A1", "A1", "B", "B", "B", "B", "B"],
    &["A1", "A1", "A1", "C", "C", "C", "C", "C"],
    &["A1", "A1", "A1", "D", "D", "D", "D", "D"],
    &["A1", "A1", "A1", "C", "C", "C", "C", "C"],
    &["A1", "A1", "A1", "A1", "A1", "A2", "A2", "A2"],
    &["A1", "A1", "A1", "A1", "A1", "A2", "A2", "A2", "A2", "A2"],
    &["A1", "A1", "A1", "A1", "B", "B", "B", "B", "C"],
];

/// Frame A's column one ends here (x, page space): it starts at 72 and is
/// 100 pt wide, with a 20 pt gutter. Frame B starts at y 300.
const COLUMN_SPLIT_X: f32 = 182.0;
const FRAME_B_TOP: f32 = 300.0;

fn open_sans() -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts/OpenSans.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read OpenSans.ttf: {e}"))
}

fn build() -> pipeline::BuiltDocument {
    let bytes = paged_gen::write_idml(&paged_gen::samples::table_start_row::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = open_sans();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&doc, &opts).expect("build")
}

/// The frame the engine set row `row` of case `case` in: its first line's
/// page and position, named as in [`INDESIGN`].
fn region(built: &pipeline::BuiltDocument, case: usize, row: u32) -> String {
    let addr = CellAddr {
        table_id: table_id(case as u32),
        row,
        col: 0,
    };
    let story = body_story_id(case as u32);
    for (p, page) in built.pages.iter().enumerate() {
        let Some(line) = page
            .story_layout
            .iter()
            .find(|l| l.story_id == story && l.cell.as_ref() == Some(&addr))
        else {
            continue;
        };
        let x = line
            .clusters
            .iter()
            .map(|c| c.x_pt)
            .fold(f32::INFINITY, f32::min);
        return match p.checked_sub(case * PAGES_PER_CASE) {
            Some(0) if line.baseline_y_pt > FRAME_B_TOP => "B".into(),
            Some(0) if x < COLUMN_SPLIT_X => "A1".into(),
            Some(0) => "A2".into(),
            Some(1) => "C".into(),
            Some(2) => "D".into(),
            Some(3) => "E".into(),
            _ => format!("page {}", p + 1),
        };
    }
    "overset".into()
}

#[test]
fn every_row_starts_where_indesign_starts_it() {
    let built = build();
    let mut report = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let got: Vec<String> = (0..INDESIGN[i].len() as u32)
            .map(|r| region(&built, i, r))
            .collect();
        if got != INDESIGN[i] {
            report.push(format!(
                "{}\n    engine   {got:?}\n    InDesign {:?}",
                case.name, INDESIGN[i]
            ));
        }
    }
    assert!(report.is_empty(), "\n{}", report.join("\n"));
}

/// The header repeats in frame B over the rows the rule moved there.
#[test]
fn the_header_repeats_over_the_moved_rows() {
    let built = build();
    let case = cases().iter().position(|c| c.header).expect("header case");
    let addr = CellAddr {
        table_id: table_id(case as u32),
        row: 0,
        col: 0,
    };
    let story = body_story_id(case as u32);
    let page = &built.pages[case * PAGES_PER_CASE];
    let in_b = page
        .story_layout
        .iter()
        .filter(|l| l.story_id == story && l.cell.as_ref() == Some(&addr))
        .any(|l| l.baseline_y_pt > FRAME_B_TOP);
    assert!(in_b, "no header line in frame B");
}
