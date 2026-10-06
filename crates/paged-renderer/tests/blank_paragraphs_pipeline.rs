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

//! A paragraph with no word still takes a line, as InDesign 20.0.1 set
//! the `blank-paragraphs` paged-gen fixture (`corpus/generated/
//! blank-paragraphs.pdf`, 2026-10-02): "A", something, "B" per frame, and
//! B's distance below A says how tall the something is.

use paged_gen::samples::blank_paragraphs::{cases, story_id};
use paged_renderer::{pipeline, PipelineOptions};

/// InDesign's distance from A's baseline to B's, per frame (pt). A
/// paragraph of spaces is a line at its own leading: 12 + 12, and
/// 24 + 12 for the space set 20/24.
const INDESIGN: [f32; 6] = [12.0, 24.0, 24.0, 24.0, 36.0, 36.0];

/// Frames where the engine still differs: `(frame, the engine's distance)`.
/// - 1, the EMPTY paragraph: the IDML importer drops a character range
///   with no text, so the blank line loses the range's 12 pt leading and
///   takes auto leading (1.2 x the default 12 pt). RFI C-57; the Word
///   import is not affected (it styles blank lines by paragraph style).
const KNOWN: [(usize, f32); 1] = [(1, 26.4)];

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn a_paragraph_of_spaces_is_a_line_as_in_indesign() {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::blank_paragraphs::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let mut wrong = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let lines = built.story_layout(&story_id(i as u32));
        let words: Vec<f32> = lines
            .iter()
            .filter(|l| !l.byte_range.is_empty())
            .map(|l| l.baseline_y_pt)
            .collect();
        // "A" and "B": a line of spaces has bytes and no ink, so take the
        // first and the last line.
        let (a, b) = (words[0], *words.last().expect("B"));
        if let Some((_, known)) = KNOWN.iter().find(|(k, _)| *k == i) {
            assert!(
                (b - a - known).abs() < 0.05,
                "{}: changed to {:.2}: update or drop its KNOWN entry",
                case.name,
                b - a
            );
            continue;
        }
        if (b - a - INDESIGN[i]).abs() > 0.05 {
            wrong.push(format!(
                "{}: B is {:.2} pt below A, InDesign {:.2}",
                case.name,
                b - a,
                INDESIGN[i]
            ));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
