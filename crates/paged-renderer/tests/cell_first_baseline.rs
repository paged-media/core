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

//! A table cell sets its first baseline by its own `FirstBaselineOffset`
//! and `MinimumFirstBaselineOffset`, pinned against InDesign 20.0.1's
//! geometry for `table-cell-baseline.idml` (every cell's first baseline
//! and three frames' first lines, read through its DOM by
//! `tools/indesign-export/cells-probe.sh`; tolerance 0.05 pt).
//!
//! What InDesign answered:
//! * ascent, cap height, x height, the leading and `FixedHeight` (the
//!   minimum) place a cell's first line as they place a frame's;
//! * the em box's top stands `(cap height + 1 em) / 2` above the
//!   baseline — in a cell and in a frame alike (the point size the
//!   renderer used is the box's height, not its top); InDesign spells it
//!   `EmboxHeight`;
//! * `MinimumFirstBaselineOffset` floors every policy in a cell, not only
//!   the leading;
//! * a centred cell centres the block from the first line's top (its
//!   offset above the baseline) to its baseline, whatever the policy;
//! * a cell style's policy applies exactly as the cell's own (page 2 =
//!   page 1 in InDesign); the IDML reader does not take it off a
//!   `<CellStyle>` yet, so page 2 is pinned behind `#[ignore]`.

use std::path::PathBuf;

use paged_gen::samples::table_cell_baseline as sample;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

const TOL: f32 = 0.05;

/// InDesign's first baselines, page pt from the page top, by row (the
/// header row, then one row per `sample::POLICIES` entry) and column
/// (the label column, then one per `sample::COLUMNS` entry). The same
/// on both pages.
const INDESIGN: [[f32; 5]; 10] = [
    [93.051, 93.051, 93.051, 93.051, 93.051],
    [117.051, 126.377, 141.438, 131.877, 144.188],
    [173.051, 175.277, 193.889, 180.777, 196.639],
    [229.051, 227.703, 248.102, 233.203, 250.852],
    [285.051, 297.000, 310.750, 302.500, 313.500],
    [341.051, 346.139, 363.319, 351.639, 366.069],
    [397.051, 415.000, 425.750, 420.500, 428.500],
    [453.051, 471.000, 481.750, 476.500, 484.500],
    [509.051, 527.000, 537.750, 532.500, 540.500],
    [565.051, 570.000, 587.250, 575.500, 590.000],
];

/// InDesign's first baselines of the frames below page 1's table, in
/// `sample::FRAME_CASES` order.
const INDESIGN_FRAMES: [f32; 3] = [697.139, 697.0, 700.12];

fn read_font(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

fn build() -> pipeline::BuiltDocument {
    let bytes = paged_gen::write_idml(&sample::build()).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open");
    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    resolver.add_font("Lora", None, read_font("Lora.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).expect("build_document")
}

/// Every cell's first baseline on `page`, as `[row][column]`.
fn cell_baselines(built: &pipeline::BuiltDocument, page: usize) -> Vec<Vec<Option<f32>>> {
    let mut grid = vec![vec![None; 5]; INDESIGN.len()];
    for line in &built.pages[page].story_layout {
        let Some(cell) = line.cell.as_ref() else {
            continue;
        };
        if line.paragraph_idx != 0 || line.line_idx != 0 {
            continue;
        }
        let slot = &mut grid[cell.row as usize][cell.col as usize];
        if slot.is_none() {
            *slot = Some(line.baseline_y_pt);
        }
    }
    grid
}

fn assert_cells_as_indesign(page: usize) {
    let built = build();
    let grid = cell_baselines(&built, page);
    let mut misses = Vec::new();
    for (row, expected) in INDESIGN.iter().enumerate() {
        for (col, &want) in expected.iter().enumerate() {
            // The corner cell is empty: no line, nothing to place.
            if (row, col) == (0, 0) {
                continue;
            }
            let got = grid[row][col];
            if got.is_none_or(|g| (g - want).abs() > TOL) {
                let what = match (row, col) {
                    (0, _) => "header".to_string(),
                    (r, 0) => format!("label {r}"),
                    (r, c) => format!(
                        "{} / {}",
                        sample::POLICIES[r - 1].label,
                        sample::COLUMNS[c - 1].label
                    ),
                };
                misses.push(format!("{what}: InDesign {want}, ours {got:?}"));
            }
        }
    }
    assert!(
        misses.is_empty(),
        "page {}:\n{}",
        page + 1,
        misses.join("\n")
    );
}

#[test]
fn a_cell_sets_its_first_baseline_by_its_own_policy() {
    assert_eq!(sample::PAGES[0], sample::Source::Cell);
    assert_cells_as_indesign(0);
}

#[test]
#[ignore = "plugin-publish idml-import does not read CellStyle@FirstBaselineOffset"]
fn a_cell_style_sets_the_first_baseline_as_the_cell_would() {
    assert_eq!(sample::PAGES[1], sample::Source::CellStyle);
    assert_cells_as_indesign(1);
}

/// The em box in a frame, with two faces, and the ascent beside it.
#[test]
fn a_frame_sets_the_em_box_where_indesign_does() {
    let built = build();
    let page = &built.pages[0];
    let top = sample::FRAME_CASE_AT.1;
    for (i, &want) in INDESIGN_FRAMES.iter().enumerate() {
        let x = sample::FRAME_CASE_AT.0 + sample::FRAME_CASE_STEP * i as f32;
        let line = page
            .story_layout
            .iter()
            .filter(|l| l.cell.is_none() && l.baseline_y_pt > top)
            .find(|l| l.clusters.first().is_some_and(|c| (c.x_pt - x).abs() < 1.0))
            .unwrap_or_else(|| panic!("no line in frame case {i}"));
        assert!(
            (line.baseline_y_pt - want).abs() <= TOL,
            "{:?}: InDesign {want}, ours {}",
            sample::FRAME_CASES[i],
            line.baseline_y_pt,
        );
    }
}
