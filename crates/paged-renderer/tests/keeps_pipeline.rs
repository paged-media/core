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

//! ADR 028 — paragraph keep options over the generated `keeps.idml`.
//!
//! Each page is a two-column frame on an exact ten-lines-per-column grid
//! (`paged_gen::samples::keeps`). The expected splits are InDesign 2025's
//! own, read from its export of this fixture (`corpus/generated/keeps.pdf`,
//! 2026-10-01), so this pins the engine to InDesign rather than to a
//! reading of the rules.

use std::path::PathBuf;

use paged_renderer::{pipeline, PipelineOptions};

/// `(lines in column one, lines in column two)` per page, as InDesign
/// exported them.
const INDESIGN: [(&str, usize, usize); 8] = [
    ("control · one-line paragraph at the column foot", 10, 6),
    ("KeepWithNext=2 moves the one-line paragraph", 9, 7),
    ("control · three-line paragraph ends at the foot", 10, 6),
    ("KeepWithNext=1 moves only the LAST line (2 | 1)", 9, 7),
    ("control · four-line paragraph straddles 2 | 2", 10, 7),
    ("KeepAllLinesTogether moves the whole paragraph", 8, 9),
    ("KeepFirstLines=2: 1 | 3 moves the whole paragraph", 9, 8),
    ("KeepLastLines=2: 3 | 1 becomes 2 | 2", 9, 8),
];

/// Column two starts 120 pt right of column one (100 pt column + 20 pt
/// gutter); anything left of the midpoint is column one.
const COLUMN_SPLIT_X: f32 = 72.0 + 110.0;

fn inter_font() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn keep_options_break_columns_where_indesign_does() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::keeps::build()).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build_document");

    let mut mismatches = Vec::new();
    for (seq, (case, want_one, want_two)) in INDESIGN.iter().enumerate() {
        let story = paged_gen::ids::self_id("keeps", "BodyStory", seq as u32);
        let lines = built.story_layout(&story);
        let one = lines
            .iter()
            .filter(|l| {
                l.clusters
                    .iter()
                    .map(|c| c.x_pt)
                    .fold(f32::INFINITY, f32::min)
                    < COLUMN_SPLIT_X
            })
            .count();
        let two = lines.len() - one;
        if (one, two) != (*want_one, *want_two) {
            mismatches.push(format!(
                "page {seq} ({case}): engine {one} | {two}, InDesign {want_one} | {want_two}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "column breaks differ from InDesign:\n  {}",
        mismatches.join("\n  ")
    );
}
