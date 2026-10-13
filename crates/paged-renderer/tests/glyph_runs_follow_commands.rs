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

//! A glyph-run entry keeps its own copy of its outline command's
//! transform: the PDF writer sets the text matrix from it. Every pass that
//! moves commands after they are emitted — vertical justification, a
//! rotated frame, a rotated story, a rotated cell — must move the entries
//! with them. When vertical justification did not, every vertically
//! centred frame exported its text at the top of the frame
//! (`transparency-targets`, whose twelve text frames are centred).

use paged_renderer::{pipeline, PipelineOptions};

fn check(name: &str, sample: paged_gen::Sample) {
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let font = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/fonts/Inter.ttf"
    ))
    .unwrap();
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).unwrap();
    let mut checked = 0;
    for (p, page) in built.pages.iter().enumerate() {
        let Some(table) = page.list.glyph_runs.as_ref() else {
            continue;
        };
        for e in &table.entries {
            let cmd = &page.list.commands[e.command_index as usize];
            let mut cmd = cmd.clone();
            assert_eq!(
                *cmd.transform_mut(),
                e.transform,
                "{name} page {p}: glyph {} drifted from its command",
                e.glyph_id
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "{name}: no glyph runs captured");
}

#[test]
fn vertically_justified_frames_move_their_glyph_runs() {
    check(
        "transparency-targets",
        paged_gen::samples::transparency_targets::build(),
    );
    check("text", paged_gen::samples::text::build());
}

#[test]
fn rotated_frames_and_cells_move_their_glyph_runs() {
    check("geometry", paged_gen::samples::geometry::build());
    check("tables", paged_gen::samples::tables::build());
    check("text-advanced", paged_gen::samples::text_advanced::build());
}
